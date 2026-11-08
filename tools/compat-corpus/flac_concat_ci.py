"""Current fixture CI policy. Never use this adapter to judge a frozen round."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path[:0] = [str(HERE), str(ROOT / 'scripts')]

import copy_check
import decoded_output
import join_check
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

LEGACY_ORACLE = copy_check.oracle
MAX_PACKET = 16 << 20
MAX_PACKETS = 1 << 16
FROZEN = ('scripts/clip_check.py', 'scripts/join_check.py', 'tools/compat-corpus/copy_check.py')
FIXTURE = 'av_flac.mp4'
FIXTURE_SHA256 = 'bbee8fc6964fd11386c397cf68238ac77e4695f5ab7dda5eee7ca4801ca17723'
VARIANTS = (
    ('-f', 'concat', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'),
    ('-f', 'concat', '-safe', '0', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'),
    ('-safe', '0', '-f', 'concat', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'),
    ('-y', '-f', 'concat', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'),
    ('-f', 'concat', '-i', 'INPUT', '-vcodec', 'copy', '-acodec', 'copy', 'OUTPUT'),
    ('-y', '-safe', '0', '-f', 'concat', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'),
)
FROZEN_DIFFERENCES = [
    'audio: 174 packets differ, first (input, packet) [(0, 0), (0, 1), (0, 2)]',
    "audio: 174 packets differ from ffmpeg concat's, first 0",
]


def frozen_check(root=ROOT):
    expected = json.loads((root / 'docs/measurements/compat-0.1.0/judging.json').read_text())['files']
    if any(hashlib.sha256((root / name).read_bytes()).hexdigest() != expected[name] for name in FROZEN):
        raise ValueError('stage=frozen_mismatch')


def crc(data, width, polynomial):
    value = 0
    mask = (1 << width) - 1
    for byte in data:
        value ^= byte << (width - 8)
        for _ in range(8):
            value = ((value << 1) ^ (polynomial if value & (1 << (width - 1)) else 0)) & mask
    return value


def frame(data):
    if not 8 <= len(data) <= MAX_PACKET or data[0] != 255 or data[1] & 254 != 248:
        raise ValueError('FLAC frame extent or sync')
    block_code, rate_code = data[2] >> 4, data[2] & 15
    if data[3] & 1 or data[3] >> 4 > 10 or (data[3] >> 1) & 7 == 3 or block_code == 0 or rate_code == 15:
        raise ValueError('FLAC reserved field')
    first = data[4]
    count = 1 if first < 128 else next((n for n in range(2, 8) if first >> (8 - n) == (1 << n) - 1
                                                        and not first & (1 << (7 - n))), 0)
    if count == 0 or 4 + count > len(data) - 3:
        raise ValueError('FLAC coded number')
    number = first if count == 1 else first & ((1 << (7 - count)) - 1)
    for byte in data[5:4 + count]:
        if byte & 192 != 128:
            raise ValueError('FLAC coded number continuation')
        number = number * 64 + (byte & 63)
    minimum = 0 if count == 1 else 128 if count == 2 else 1 << (5 * count - 4)
    variable = bool(data[1] & 1)
    if number < minimum or number >= 1 << (36 if variable else 31):
        raise ValueError('FLAC coded number range')
    number_end = at = 4 + count
    if block_code in (6, 7):
        width = block_code - 5
        block = int.from_bytes(data[at:at + width], 'big') + 1
        at += width
    else:
        block = 192 if block_code == 1 else 576 << (block_code - 2) if block_code <= 5 else 1 << block_code
    if not 1 <= block <= 65535:
        raise ValueError('FLAC block size')
    rate = (None, 88200, 176400, 192000, 8000, 16000, 22050, 24000, 32000, 44100, 48000, 96000)[rate_code] if rate_code < 12 else None
    if rate_code >= 12:
        width = 1 if rate_code == 12 else 2
        rate = int.from_bytes(data[at:at + width], 'big') * (1000 if rate_code == 12 else 10 if rate_code == 14 else 1)
        if rate == 0:
            raise ValueError('FLAC sample rate')
        at += width
    if at >= len(data) - 2 or at >= 16 or crc(data[:at], 8, 7) != data[at]:
        raise ValueError('FLAC header CRC')
    if crc(data, 16, 0x8005):
        raise ValueError('FLAC frame CRC')
    canonical = data[:1] + bytes([data[1] & 254]) + data[2:4] + b'\0' + data[number_end:at] + data[at + 1:-2]
    channels = (data[3] >> 4) + 1 if data[3] >> 4 <= 7 else 2
    bits = (None, 8, 12, None, 16, 20, 24, 32)[data[3] >> 1 & 7]
    return {'block': block, 'number': number, 'variable': variable, 'size': len(data),
            'rate': rate, 'channels': channels, 'bits': bits,
            'canonical': canonical}


def stream_info(data):
    if len(data) != 34:
        raise ValueError('FLAC STREAMINFO length')
    packed = int.from_bytes(data[10:18], 'big')
    return {'minimum': int.from_bytes(data[:2], 'big'), 'maximum': int.from_bytes(data[2:4], 'big'),
            'min_frame': int.from_bytes(data[4:7], 'big'), 'max_frame': int.from_bytes(data[7:10], 'big'),
            'format': packed >> 36, 'samples': packed & ((1 << 36) - 1), 'md5': data[18:]}


def validate_stream(frames, info, expected_format):
    rate, channels, bits = info['format'] >> 8, (info['format'] >> 5 & 7) + 1, (info['format'] & 31) + 1
    if (not frames or not 16 <= info['minimum'] <= info['maximum'] <= 65535
            or info['format'] != expected_format or rate == 0 or bits < 4):
        raise ValueError('FLAC output STREAMINFO')
    strategy, nominal, samples = frames[0]['variable'], frames[0]['block'], 0
    for index, item in enumerate(frames):
        final = index + 1 == len(frames)
        if (item['variable'] != strategy or item['number'] != (samples if strategy else index)
                or item['rate'] not in (None, rate) or item['bits'] not in (None, bits) or item['channels'] != channels
                or item['block'] > info['maximum'] or not final and item['block'] < info['minimum']
                or not strategy and (item['block'] > nominal or not final and item['block'] != nominal)):
            raise ValueError('FLAC output continuity or block bounds')
        samples += item['block']
    sizes = [item['size'] for item in frames]
    minimum, maximum = (value if value < 1 << 24 else 0 for value in (min(sizes), max(sizes)))
    if (strategy and info['minimum'] == info['maximum'] or samples >= 1 << 36 or info['samples'] != samples
            or info['md5'] != bytes(16) or info['min_frame'] != minimum or info['max_frame'] != maximum):
        raise ValueError('FLAC output statistics')


def packet_key(packet):
    return int(packet['pos']), int(packet['size']), packet['data_hash']


def stream_projection(ffprobe, path, index, reader, output, expected_format):
    description = join_check.cc.ffprobe_json(ffprobe, '-select_streams', str(index), '-show_streams', '-show_data', path)
    streams = description.get('streams', [])
    if len(streams) != 1 or streams[0].get('codec_name') != 'flac':
        return None
    packets = reader(ffprobe, path, index, raw=True)
    if not packets or len(packets) > MAX_PACKETS:
        raise ValueError('FLAC packet count')
    frames, projection = [], {}
    with open(path, 'rb') as source:
        extent = os.fstat(source.fileno()).st_size
        previous_end = total = 0
        for packet in packets:
            offset, size, digest = packet_key(packet)
            total += size
            if not 0 < size <= MAX_PACKET or total > 128 << 20 or offset < previous_end or offset + size > extent:
                raise ValueError('FLAC packet extent')
            source.seek(offset)
            data = source.read(size)
            if len(data) != size or digest != 'SHA256:' + hashlib.sha256(data).hexdigest():
                raise ValueError('FLAC packet identity')
            item = frame(data)
            canonical = item.pop('canonical')
            frames.append(item)
            projection[packet_key(packet)] = (len(canonical), 'SHA256:' + hashlib.sha256(canonical).hexdigest())
            previous_end = offset + size
    if Path(path).resolve() == output:
        info = stream_info(decoded_output.hexdump(streams[0].get('extradata', '')))
        validate_stream(frames, info, expected_format)
    return projection


@contextlib.contextmanager
def projected_packets(output, expected_format):
    reader = join_check.cc.packets
    cache = {}

    def packets(ffprobe, path, index, raw=False):
        key = str(Path(path).resolve()), index
        if key not in cache:
            cache[key] = stream_projection(ffprobe, path, index, reader, Path(output).resolve(), expected_format)
        rows = reader(ffprobe, path, index, raw=raw)
        projection = cache[key]
        if projection is None:
            return rows
        result = []
        for packet in rows:
            size, digest = projection[packet_key(packet)]
            result.append({**packet, 'size': str(size), 'data_hash': digest})
        return result

    join_check.cc.packets = packets
    try:
        yield
    finally:
        join_check.cc.packets = reader


def qualified_format(record, tools):
    if record.get('job') != 'join' or Path(record['source']).name != FIXTURE:
        return None
    if (record.get('id') != 'C02' or record.get('status') != 'written'
            or tuple(record['args']) not in VARIANTS):
        raise ValueError('stage=unlisted_flac_concat_case')
    source = Path(record['source'])
    if source.stat().st_size > 128 << 20 or copy_check.digest([source]) != FIXTURE_SHA256:
        raise ValueError('stage=flac_fixture_digest')
    streams = copy_check.streams(tools['ffprobe'], record['source']) or []
    audio = [stream for stream in streams if stream.get('codec_type') == 'audio']
    video = [stream for stream in streams if stream.get('codec_type') == 'video']
    if len(streams) != 2 or len(audio) != 1 or len(video) != 1 or audio[0].get('codec_name') != 'flac':
        raise ValueError('stage=flac_fixture_streams')
    listing = Path(record['input'])
    line = "file '" + source.name + "'\n"
    if listing.stat().st_size > 4096 or listing.read_text() != line * 2:
        raise ValueError('stage=flac_fixture_list')
    listed = listing.parent / source.name
    if (not os.path.samefile(source, listed)
            and (max(source.stat().st_size, listed.stat().st_size) > 128 << 20
                 or copy_check.digest([source]) != copy_check.digest([listed]))):
        raise ValueError('stage=flac_fixture_binding')
    inputs = copy_check.entry(record, None, [])['inputs']
    if len(inputs) != 2 or any(Path(item['path']).resolve() != listed.resolve()
                               or item['audio_stream'] != audio[0]['index']
                               or item['video_stream'] != video[0]['index'] for item in inputs):
        raise ValueError('stage=flac_fixture_inputs')
    config = join_check.cc.extradata(tools['ffprobe'], record['source'], audio[0]['index'])
    return stream_info(decoded_output.hexdump(config))['format']


def pcm_proof(record, tools):
    limit = 16 << 20
    with tempfile.TemporaryDirectory(prefix='flac-concat-ci-') as temporary:
        decoded = []
        for index, source in enumerate((record['source'], record['output'])):
            path = Path(temporary) / str(index)
            command = [tools['ffmpeg'], '-v', 'error', '-nostdin', '-xerror', '-err_detect', 'crccheck+explode',
                       '-ignore_editlist', '1', *AUDIO_DECODE_INPUT, '-i', str(source), '-map', '0:a:0',
                       '-c:a', 'pcm_s32le', '-f', 's32le', '-fs', str(limit), '-']
            code, _, error = decoded_output.invoke(command, os.environ.copy(), output=path)
            if code != 0 or error or not 0 < path.stat().st_size < limit:
                raise ValueError('stage=flac_pcm_decode')
            decoded.append(path.read_bytes())
        if decoded[1] != decoded[0] * 2:
            raise ValueError('stage=flac_concatenated_input_pcm')


def exact_cases(records):
    cases = [record for record in records if record.get('job') == 'join'
             and Path(record['source']).name == FIXTURE]
    if (len(cases) != len(VARIANTS) or {tuple(record['args']) for record in cases} != set(VARIANTS)
            or any(record.get('id') != 'C02' or record.get('status') != 'written' for record in cases)):
        raise ValueError('stage=flac_exact_six_cases')
    return cases


def current_oracle(record, tools):
    expected_format = qualified_format(record, tools)
    if expected_format is None:
        return LEGACY_ORACLE(record, tools)
    original = LEGACY_ORACLE(record, tools)
    if original != FROZEN_DIFFERENCES:
        return original or ['stage=flac_frozen_identity_returned']
    pcm_proof(record, tools)
    with projected_packets(record['output'], expected_format):
        return LEGACY_ORACLE(record, tools)


def main():
    frozen_check()
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--records', required=True)
    parser.add_argument('--ffprobe', default='ffprobe')
    args, _ = parser.parse_known_args()
    for record in exact_cases(json.loads(Path(args.records).read_text())):
        qualified_format(record, {'ffprobe': args.ffprobe})
    copy_check.oracle = current_oracle
    return copy_check.main()


if __name__ == '__main__':
    sys.exit(main())
