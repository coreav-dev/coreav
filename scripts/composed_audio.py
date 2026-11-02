"""The registered composed Opus-to-AAC predicate; historical B9 is unchanged."""
import ast
import dataclasses
from fractions import Fraction
import hashlib
import json
import math
import os
import pathlib
import re
import selectors
import signal
import shutil
import struct
import subprocess
import sys
import time

from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT


ROOT = pathlib.Path(__file__).resolve().parent.parent
REGISTRATION = ROOT / 'docs/design/composed-opus-aac-v1.json'
VERSION = 'composed-opus-aac-v1'
MAX_CONTROL = 1 << 20
MAX_BOXES = 4096
MAX_METADATA = 16 << 20


class Invalid(ValueError):
    pass


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Invalid('duplicate JSON key')
        result[key] = value
    return result


def json_document(data):
    def refuse_constant(_):
        raise Invalid('nonfinite JSON')

    try:
        value = json.loads(data, object_pairs_hook=unique_pairs, parse_constant=refuse_constant)
        if not isinstance(value, dict):
            raise Invalid('JSON document must be an object')
        return value
    except (UnicodeError, ValueError) as error:
        raise Invalid('invalid JSON document') from error


def sha256_file(path):
    with open(path, 'rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def registration():
    data = REGISTRATION.read_bytes()
    value = json_document(data)
    if value.get('schema') != 'coreav-transcode-check-registration/1' or value.get('version') != VERSION:
        raise Invalid('unknown composed-check registration')
    return value, {'version': VERSION, 'registration_sha256': hashlib.sha256(data).hexdigest()}


def method_digest():
    sources = ('scripts/composed_audio.py', 'scripts/transcode_check.py', 'scripts/oracle_audio.py',
               'scripts/oracle-audio-args.txt', 'scripts/oracle_pin.py', 'scripts/composed_audio_fixtures.py',
               'docs/design/composed-opus-aac-v1.json')
    return hashlib.sha256(json.dumps({path: sha256_file(ROOT / path) for path in sources}, sort_keys=True).encode()).hexdigest()


def method_identity():
    current = method_digest()
    if current != LOADED_METHOD:
        raise Invalid('judging source bytes changed after the checker was loaded')
    return current


def check_policy():
    value, identity = registration()
    legacy = value['legacy']
    source = (ROOT / legacy['file']).read_text()
    functions = [node for node in ast.parse(source).body
                 if isinstance(node, ast.FunctionDef) and node.name == legacy['function']]
    if len(functions) != 1:
        raise Invalid('legacy encoded-audio predicate not found')
    node = functions[0]
    function = ''.join(source.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])
    policy = (ROOT / value['decoder']['policy']).read_text().split('### Opus bars\n', 1)[1].split('3. **Speed.**', 1)[0]
    actual = (hashlib.sha256(function.encode()).hexdigest(), hashlib.sha256(policy.encode()).hexdigest(),
              sha256_file(ROOT / value['decoder']['conversion_owner']),
              sha256_file(ROOT / value['encoder']['pin']))
    expected = (legacy['function_sha256'], value['decoder']['policy_sha256'],
                value['decoder']['conversion_sha256'], value['encoder']['pin_sha256'])
    if actual != expected:
        raise Invalid('registered legacy or decoder/oracle policy changed')
    return identity


def require_version(document, identity):
    if not isinstance(document, dict) or document.get('compat_check') != identity:
        raise Invalid('missing, mixed or relabeled check version')


def integer(value, name, minimum=0):
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise Invalid('invalid ' + name)
    return value


@dataclasses.dataclass(frozen=True)
class Edits:
    track_id: int
    movie_scale: int
    media_scale: int
    entries: tuple
    media_spans: tuple
    channels: int = 1


class Headers:
    def __init__(self, path):
        self.file = open(path, 'rb')
        self.file.seek(0, 2)
        self.size = self.file.tell()
        self.boxes = 0
        self.control = 0

    def read(self, start, length):
        self.control += length
        if length < 0 or self.control > MAX_CONTROL:
            raise Invalid('MP4 control bytes exceed their limit')
        self.file.seek(start)
        data = self.file.read(length)
        if len(data) != length:
            raise Invalid('truncated MP4 control box')
        return data

    def children(self, start, end):
        result = {}
        while start < end:
            self.boxes += 1
            if self.boxes > MAX_BOXES or end - start < 8:
                raise Invalid('invalid or excessive MP4 boxes')
            length, kind = struct.unpack('>I4s', self.read(start, 8))
            header = 8
            if length == 1:
                if end - start < 16:
                    raise Invalid('truncated extended MP4 box')
                length = struct.unpack('>Q', self.read(start + 8, 8))[0]
                header = 16
            elif length == 0:
                length = end - start
            if length < header or length > end - start:
                raise Invalid('MP4 box outside its parent')
            if kind in (b'cmov', b'moof', b'mvex'):
                raise Invalid('compressed or fragmented MP4 timing is not registered')
            result.setdefault(kind, []).append((start + header, start + length))
            start += length
        return result

    def one(self, children, kind):
        boxes = children.get(kind, [])
        if len(boxes) != 1:
            raise Invalid('missing or duplicate MP4 timing box')
        return boxes[0]

    def payload(self, box):
        start, end = box
        return self.read(start, end - start)

    def timed_header(self, box, lengths):
        data = self.payload(box)
        if not data or data[0] not in (0, 1) or len(data) != lengths[data[0]]:
            raise Invalid('unsupported or malformed MP4 time header')
        return data

    def selected(self, track_id):
        top = self.children(0, self.size)
        spans = tuple(top.get(b'mdat', []))
        if not spans:
            raise Invalid('MP4 has no media payload')
        movie = self.children(*self.one(top, b'moov'))
        mvhd = self.timed_header(self.one(movie, b'mvhd'), (100, 112))
        movie_scale = struct.unpack_from('>I', mvhd, 12 if mvhd[0] == 0 else 20)[0]
        tracks = {}
        for box in movie.get(b'trak', []):
            track = self.children(*box)
            tkhd = self.timed_header(self.one(track, b'tkhd'), (84, 96))
            identity = struct.unpack_from('>I', tkhd, 12 if tkhd[0] == 0 else 20)[0]
            if identity == 0 or identity in tracks:
                raise Invalid('duplicate or zero MP4 track identity')
            tracks[identity] = track
        if track_id not in tracks:
            raise Invalid('ffprobe stream has no matching MP4 track')
        track = tracks[track_id]
        media = self.children(*self.one(track, b'mdia'))
        handler = self.payload(self.one(media, b'hdlr'))
        if len(handler) < 24 or handler[:4] != b'\0' * 4 or handler[8:12] != b'soun':
            raise Invalid('selected MP4 track is not audio')
        mdhd = self.timed_header(self.one(media, b'mdhd'), (24, 36))
        media_scale = struct.unpack_from('>I', mdhd, 12 if mdhd[0] == 0 else 20)[0]
        if not movie_scale or media_scale != 48000:
            raise Invalid('unregistered MP4 timing scale')
        info = self.children(*self.one(media, b'minf'))
        tables = self.children(*self.one(info, b'stbl'))
        if b'ctts' in tables or b'cslg' in tables:
            raise Invalid('audio composition offsets are not registered')
        for kind in (b'stts', b'stsc', b'stsz'):
            self.one(tables, kind)
        if len(tables.get(b'stco', [])) + len(tables.get(b'co64', [])) != 1:
            raise Invalid('missing or duplicate AAC chunk offsets')
        stsd = self.one(tables, b'stsd')
        prefix = self.read(stsd[0], 8)
        if prefix != b'\0\0\0\0\0\0\0\1':
            raise Invalid('audio needs one sample description')
        descriptions = self.children(stsd[0] + 8, stsd[1])
        if set(descriptions) != {b'mp4a'}:
            raise Invalid('selected track is not an ordinary AAC sample entry')
        entry = self.one(descriptions, b'mp4a')
        if entry[1] - entry[0] < 28:
            raise Invalid('truncated AAC sample entry')
        audio = self.read(entry[0], 28)
        channels = struct.unpack_from('>H', audio, 16)[0]
        if audio[8:10] != b'\0\0' or struct.unpack_from('>I', audio, 24)[0] != 48000 << 16:
            raise Invalid('unregistered AAC sample entry format')
        if channels not in (1, 2):
            raise Invalid('unregistered AAC sample entry channels')
        self.one(self.children(entry[0] + 28, entry[1]), b'esds')
        edits = self.children(*self.one(track, b'edts'))
        if set(edits) != {b'elst'}:
            raise Invalid('unregistered edit container')
        data = self.payload(self.one(edits, b'elst'))
        if len(data) < 8 or data[0] not in (0, 1) or data[1:4] != b'\0' * 3:
            raise Invalid('unsupported edit list version')
        count = struct.unpack_from('>I', data, 4)[0]
        width, fmt = (12, '>Iihh') if data[0] == 0 else (20, '>Qqhh')
        if count not in (1, 2) or len(data) != 8 + count * width:
            raise Invalid('unsupported or truncated edit list')
        entries = tuple(struct.unpack_from(fmt, data, 8 + i * width) for i in range(count))
        if any(rate != 1 or fractional != 0 or duration == 0 for duration, _, rate, fractional in entries):
            raise Invalid('non-unit or empty edit')
        if entries[-1][1] < 0 or (count == 2 and entries[0][1] != -1):
            raise Invalid('only an optional empty edit and one media edit are registered')
        return Edits(track_id, movie_scale, media_scale, entries, spans, channels)


def read_edits(path, track_id):
    reader = Headers(path)
    try:
        return reader.selected(track_id)
    finally:
        reader.file.close()


def sample_map(edits, packets, frames):
    integer(frames, 'capture frames', 1)
    rate, frame = edits.media_scale, 1024
    if rate != 48000 or not isinstance(packets, list) or not packets:
        raise Invalid('missing registered AAC packets')
    cap = (frames + 2048 + frame - 1) // frame + 1
    if len(packets) > cap:
        raise Invalid('too many AAC packets')
    origin = Fraction(edits.entries[0][0] * rate, edits.movie_scale) if len(edits.entries) == 2 else Fraction(0)
    duration, priming, _, _ = edits.entries[-1]
    length = Fraction(duration * rate, edits.movie_scale)
    if origin.denominator != 1 or length.denominator != 1 or length != frames or not 0 <= priming <= 2048:
        raise Invalid('edit does not describe the exact captured sample grid')
    covered = 0
    for ordinal, packet in enumerate(packets):
        pts = integer(packet.get('pts'), 'AAC PTS')
        dts = integer(packet.get('dts'), 'AAC DTS')
        duration = integer(packet.get('duration'), 'AAC duration', 1)
        position = integer(packet.get('pos'), 'AAC position')
        size = integer(packet.get('size'), 'AAC packet size', 1)
        if pts != ordinal * frame or dts != pts or duration > frame or (ordinal + 1 < len(packets) and duration != frame):
            raise Invalid('interior AAC sample timestamp or duration differs')
        if not any(start <= position and position + size <= end for start, end in edits.media_spans):
            raise Invalid('AAC packet is outside media bytes')
        start, end = max(pts, priming), min(pts + duration, priming + frames)
        if end > start:
            if start - priming != covered:
                raise Invalid('AAC presentation has a hole, overlap or displacement')
            covered = end - priming
    if covered != frames:
        raise Invalid('AAC packets do not cover the captured samples')
    return {'start_ticks': int(origin), 'frames': frames, 'rate': rate, 'priming': priming,
            'packets_checked': len(packets)}


def same_presentation(candidate, baseline, reference, capture):
    frames = integer(capture.get('samples'), 'capture samples', 1)
    rate = integer(capture.get('sample_rate'), 'capture rate', 1)
    start = integer(capture.get('start_ticks'), 'capture start')
    ours, theirs, pcm = (sample_map(edits, packets, frames) for edits, packets in (candidate, baseline, reference))
    if rate != 48000 or ours['start_ticks'] != start or theirs['start_ticks'] != start or pcm['start_ticks'] != 0:
        raise Invalid('source, candidate or captured PCM presentation origin differs')
    if ours['priming'] != pcm['priming']:
        raise Invalid('candidate priming differs from the independent PCM reference')
    return dict(ours, ok=True)


def bounded(cmd, env, *, limit=MAX_METADATA, timeout=120, stdin=None, output=None, messages=False):
    stdout, stderr = bytearray(), bytearray()
    total = 0
    deadline = time.monotonic() + timeout
    environment = {k: v for k, v in env.items()
                   if k != 'CAPY_API_KEY' and not k.startswith(('R2_', 'AWS_', 'COREAV_GCP_', 'CORPUS_R2'))}
    with subprocess.Popen(list(map(str, cmd)), stdin=stdin or subprocess.DEVNULL,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment,
                          start_new_session=True) as child:
        try:
            with selectors.DefaultSelector() as selected:
                selected.register(child.stdout, selectors.EVENT_READ, 'stdout')
                selected.register(child.stderr, selectors.EVENT_READ, 'stderr')
                while selected.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise Invalid('composed reference command timed out')
                    for key, _ in selected.select(remaining):
                        data = key.fileobj.read1(65536)
                        if not data:
                            selected.unregister(key.fileobj)
                        elif key.data == 'stderr':
                            stderr.extend(data)
                            if len(stderr) > 65536:
                                raise Invalid('composed reference stderr exceeds its limit')
                        else:
                            total += len(data)
                            if total > limit:
                                raise Invalid('composed reference output exceeds its limit')
                            if output is None:
                                stdout.extend(data)
                            else:
                                output.write(data)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise Invalid('composed reference command timed out')
            code = child.wait(timeout=remaining)
            if code != 0 or (stderr and not messages):
                raise Invalid('composed reference command failed')
            return bytes(stdout), bytes(stderr)
        except BaseException:
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
            raise


def decimal(value, name):
    if isinstance(value, str) and re.fullmatch(r'(?:0|[1-9][0-9]{0,19})', value):
        value = int(value)
    return integer(value, name)


def read_aac_timing(ffprobe, path, index, frames, env):
    output, _ = bounded([ffprobe, '-v', 'error', '-ignore_editlist', '1', '-select_streams', str(index),
                         '-show_packets', '-show_streams', '-show_data_hash', 'sha256', '-show_entries',
                         'packet=pts,dts,duration,pos,size,data_hash:'
                         'stream=index,id,codec_name,sample_rate,channels,time_base,extradata_hash,extradata_size',
                         '-of', 'json', path], env)
    document = json_document(output)
    streams, packets = document.get('streams'), document.get('packets')
    if not isinstance(streams, list) or len(streams) != 1 or not isinstance(streams[0], dict):
        raise Invalid('selected AAC stream metadata was not read')
    stream = streams[0]
    if stream.get('index') != index or stream.get('codec_name') != 'aac' or \
            stream.get('time_base') != '1/48000' or decimal(stream.get('sample_rate'), 'AAC rate') != 48000 or \
            stream.get('channels') not in (1, 2):
        raise Invalid('unregistered AAC stream metadata')
    track = stream.get('id')
    if not isinstance(track, str) or not re.fullmatch(r'0x[0-9a-fA-F]{1,8}', track):
        raise Invalid('AAC stream has no exact track identity')
    configuration = stream.get('extradata_hash')
    if not isinstance(configuration, str) or not re.fullmatch(r'SHA256:[0-9a-fA-F]{64}', configuration) or \
            decimal(stream.get('extradata_size'), 'AAC configuration size') < 2:
        raise Invalid('AAC configuration was not hashed')
    cap = (integer(frames, 'capture frames', 1) + 2048 + 1023) // 1024 + 1
    if not isinstance(packets, list) or not 0 < len(packets) <= cap:
        raise Invalid('AAC packet metadata is missing or excessive')
    normalized, hashes = [], []
    for packet in packets:
        if not isinstance(packet, dict):
            raise Invalid('invalid AAC packet metadata')
        normalized.append({key: decimal(packet.get(key), 'AAC ' + key)
                           for key in ('pts', 'dts', 'duration', 'pos', 'size')})
        digest = packet.get('data_hash')
        if not isinstance(digest, str) or not re.fullmatch(r'SHA256:[0-9a-fA-F]{64}', digest):
            raise Invalid('AAC packet payload was not hashed')
        hashes.append(digest.lower())
    edits = read_edits(path, int(track, 16))
    if edits.channels != stream['channels']:
        raise Invalid('AAC sample entry and decoder channel counts differ')
    timing = edits, normalized
    sample_map(*timing, frames)
    return timing, {'configuration': configuration.lower(), 'packets': hashes, 'channels': stream['channels']}


def expected_encoder(rate, channels, bitrate):
    integer(rate, 'encoder rate', 1)
    integer(channels, 'encoder channels', 1)
    args = ['-nostdin', '-hide_banner', '-loglevel', 'error', '-f', 'f32le',
            '-ar', str(rate), '-ac', str(channels), '-i', 'pipe:0', '-map', '0:a:0', '-c:a', 'aac']
    if bitrate is not None:
        args += ['-b:a', str(integer(bitrate, 'encoder bitrate', 1))]
    return args + ['-f', 'matroska', 'pipe:1']


def validate_receipt(directory, source, output, command, expected_stream, bitrate, tools):
    registered, identity = registration()
    directory = pathlib.Path(directory)
    with (directory / 'receipt.json').open('rb') as file:
        data = file.read(registered['capture']['max_receipt_bytes'] + 1)
    if len(data) > registered['capture']['max_receipt_bytes']:
        raise Invalid('capture receipt exceeds its limit')
    receipt = json_document(data)
    if receipt.get('schema') != 'coreav.encoder-input/1' or receipt.get('complete') is not True or \
            {key: receipt.get(key) for key in identity} != identity or \
            receipt.get('call') != directory.name or receipt.get('command') != command:
        raise Invalid('capture is incomplete or belongs to another invocation')
    if receipt.get('source_sha256') != sha256_file(source) or receipt.get('output_sha256') != sha256_file(output) or \
            receipt.get('candidate_sha256') != tools['coreav']:
        raise Invalid('capture source, output or candidate bytes differ')
    audio, captured = receipt.get('audio'), receipt.get('input')
    if not isinstance(audio, dict) or not isinstance(captured, dict):
        raise Invalid('capture has no resolved audio metadata')
    channels, frames = audio.get('channels'), audio.get('samples')
    integer(frames, 'captured samples', 1)
    integer(channels, 'captured channels', 1)
    integer(audio.get('stream_index'), 'resolved source stream')
    integer(audio.get('sample_rate'), 'source rate', 1)
    integer(captured.get('sample_rate'), 'encoder rate', 1)
    integer(captured.get('channel_mask'), 'channel order', 1)
    integer(captured.get('address_space'), 'address-space limit', 1)
    integer(captured.get('open_files'), 'open-file limit', 1)
    if audio.get('stream_index') != expected_stream or audio.get('codec') != 'opus' or audio.get('encoder') != 'aac' or \
            channels not in (1, 2) or isinstance(channels, bool) or audio.get('sample_rate') != 48000 or \
            captured.get('sample_rate') != 48000 or captured.get('format') != 'f32le' or \
            captured.get('channel_mask') != (4 if channels == 1 else 3):
        raise Invalid('capture stream, rate or channel order differs')
    expected = expected_encoder(48000, channels, bitrate)
    launch = captured.get('launcher_argv')
    if captured.get('encoder_sha256') != tools['ffmpeg'] or captured.get('argv') != expected or \
            captured.get('sandboxed') is not True or captured.get('address_space') != 2 << 30 or \
            captured.get('open_files') != 64 or not isinstance(launch, list) or len(launch) < 10:
        raise Invalid('encoder identity, arguments or sandbox differs')
    if launch[:4] != ['__coreav_exec', '--address-space', str(2 << 30), '--cpu-seconds'] or \
            not isinstance(launch[4], str) or not re.fullmatch(r'[1-9][0-9]{0,7}', launch[4]) or \
            launch[5:8] != ['--open-files', '64', '--'] or len(launch) != 9 + len(expected) or \
            launch[9:] != expected or not isinstance(launch[8], str) or \
            sha256_file(launch[8]) != tools['ffmpeg']:
        raise Invalid('actual launcher command differs from the registered expectation')
    pcm = captured.get('pcm')
    path = directory / 'stdin.f32'
    if not isinstance(pcm, dict) or pcm.get('path') != str(path) or \
            decimal(pcm.get('bytes'), 'captured bytes') != frames * channels * 4 or \
            not 0 < path.stat().st_size <= registered['capture']['max_pcm_bytes'] or \
            path.stat().st_size != pcm['bytes'] or pcm.get('sha256') != sha256_file(path):
        raise Invalid('captured PCM byte count or digest differs')
    return receipt, path, dict(audio, start_ticks=integer(captured.get('start_ticks'), 'capture origin'))


def project_s16(source, destination, frames, channels):
    expected = integer(frames, 'PCM frames', 1) * integer(channels, 'PCM channels', 1) * 4
    if pathlib.Path(source).stat().st_size != expected:
        raise Invalid('PCM lengths differ')
    with open(source, 'rb') as data, open(destination, 'xb') as output:
        while block := data.read(65536):
            if len(block) % 4:
                raise Invalid('truncated PCM sample')
            values = []
            for (value,) in struct.iter_unpack('<f', block):
                if not math.isfinite(value):
                    raise Invalid('nonfinite PCM sample')
                values.append(max(-32768, min(32767, round(value * 32768))))
            output.write(struct.pack('<' + 'h' * len(values), *values))


def source_mapping(stderr, audio_outputs, streams):
    text = stderr.decode('utf-8', errors='replace')
    marker = '\nStream mapping:\n'
    if text.count(marker) != 1:
        raise Invalid('ordinary baseline has no unique stream mapping')
    block = text.split(marker, 1)[1]
    mappings = {}
    for line in block.splitlines():
        if not line.startswith('  Stream #'):
            break
        match = re.fullmatch(r'  Stream #0:([0-9]+)(?:\([^\n]*\))? -> #0:([0-9]+) \([^\n]+\)', line)
        if not match:
            raise Invalid('ordinary baseline stream mapping is unsupported')
        source, output = map(int, match.groups())
        if output in mappings:
            raise Invalid('duplicate ordinary baseline stream mapping')
        mappings[output] = source
    available = {s.get('index'): s for s in streams if isinstance(s, dict)}
    result = {}
    for output in audio_outputs:
        source = mappings.get(output)
        if source not in available or available[source].get('codec_type') != 'audio':
            raise Invalid('ordinary baseline selected audio was not established')
        result[output] = source
    return result


def compare(args, directory, source, output, output_index, baseline, baseline_index, command,
            expected_stream, bitrate, legacy_compare, env):
    _, identity = registration()
    result = {'compat_check': identity, 'judging_sha256': method_identity(), 'ok': False}
    try:
        tools = {name: sha256_file(getattr(args, name)) for name in ('coreav', 'ffmpeg', 'ffprobe', 'opus_compare')}
        verify_oracle(args, env)
        verify_comparator(args.opus_compare)
        receipt, pcm, capture = validate_receipt(directory, source, output, command, expected_stream, bitrate, tools)
        frames, channels = capture['samples'], capture['channels']
        directory = pathlib.Path(directory)
        reference_pcm = directory / 'libopus.f32'
        with reference_pcm.open('xb') as sink:
            bounded([args.ffmpeg, '-nostdin', '-v', 'error', '-request_sample_fmt', 'flt', '-c:a', 'libopus',
                     *AUDIO_DECODE_INPUT, '-i', source, '-map', f'0:{expected_stream}', '-c:a', 'pcm_f32le',
                     '-f', 'f32le', 'pipe:1'], env, limit=frames * channels * 4, output=sink)
        ours_s16, reference_s16 = directory / 'captured.s16', directory / 'libopus.s16'
        project_s16(pcm, ours_s16, frames, channels)
        project_s16(reference_pcm, reference_s16, frames, channels)
        stdout, stderr = bounded([args.opus_compare, *(['-s'] if channels == 2 else []), '-r', '48000',
                                  reference_s16, ours_s16], env, limit=65536, messages=True)
        metrics = re.findall(rb'quality metric: ([^\s]+)', stdout + stderr)
        if len(metrics) != 1 or not math.isfinite(float(metrics[0])):
            raise Invalid('Opus comparator did not report one finite quality metric')
        reference = directory / 'pcm-reference.mp4'
        expected = expected_encoder(48000, channels, bitrate)
        at = expected.index('-i')
        with pcm.open('rb') as data:
            bounded([args.ffmpeg, *expected[:at], *AUDIO_DECODE_INPUT, '-i', 'pipe:0',
                     *expected[at + 2:-3], '-f', 'mp4', '-y', reference], env, stdin=data)
        candidate_timing, candidate_data = read_aac_timing(args.ffprobe, output, output_index, frames, env)
        baseline_timing, _ = read_aac_timing(args.ffprobe, baseline, baseline_index, frames, env)
        reference_timing, reference_data = read_aac_timing(args.ffprobe, reference, 0, frames, env)
        if candidate_data != reference_data:
            raise Invalid('AAC packets or configuration differ from the exact supplied PCM reference')
        properties = legacy_compare(args, output, output_index, baseline, baseline_index)
        samples = legacy_compare(args, output, output_index, reference, 0)
        if properties.get('properties_equal') is not True or samples.get('decoded_samples_equal') is not True:
            raise Invalid('AAC source properties or exact presented PCM differ')
        presentation = same_presentation(candidate_timing, baseline_timing, reference_timing, capture)
        for name, expected_hash in tools.items():
            if sha256_file(getattr(args, name)) != expected_hash:
                raise Invalid('a composed-check program changed during acquisition')
        if sha256_file(source) != receipt['source_sha256'] or sha256_file(output) != receipt['output_sha256'] or \
                sha256_file(pcm) != receipt['input']['pcm']['sha256']:
            raise Invalid('composed input, capture or output changed during acquisition')
        method_identity()
        result.update(ok=True, source_stream=expected_stream, source_sha256=receipt['source_sha256'],
                      output_sha256=receipt['output_sha256'], tools=tools,
                      receipt_sha256=sha256_file(directory / 'receipt.json'),
                      capture={'sha256': receipt['input']['pcm']['sha256'], 'frames': frames,
                               'channels': channels, 'rate': 48000, 'start_ticks': capture['start_ticks']},
                      decoder={'ok': True, 'bar': 'Opus bars/2', 'quality': float(metrics[0]),
                               'reference_sha256': sha256_file(reference_pcm),
                               'captured_s16_sha256': sha256_file(ours_s16),
                               'reference_s16_sha256': sha256_file(reference_s16)},
                      encoder={'ok': True, 'argv': expected, 'packets': len(candidate_data['packets']),
                               'configuration': candidate_data['configuration'], 'decoded_samples_equal': True},
                      presentation=presentation, properties_equal=True)
    except (Invalid, OSError, ValueError, subprocess.SubprocessError) as error:
        result['reason'] = str(error) if isinstance(error, Invalid) else type(error).__name__
    return result


def verify_comparator(program):
    registered, identity = registration()
    path = pathlib.Path(program).with_name('comparator.json')
    with path.open('rb') as data:
        raw = data.read(65537)
    if len(raw) > 65536:
        raise Invalid('comparator build metadata exceeds its limit')
    metadata = json_document(raw)
    if metadata != {'archive_sha256': registered['opus_compare']['sha256'],
                    'program_sha256': sha256_file(program),
                    'builder_sha256': sha256_file(ROOT / 'scripts/composed_audio_fixtures.py'),
                    'registration': identity}:
        raise Invalid('comparator is not bound to the pinned build recipe')


def verify_oracle(args, env):
    ffmpeg = pathlib.Path(shutil.which(args.ffmpeg) or args.ffmpeg).resolve()
    ffprobe = pathlib.Path(shutil.which(args.ffprobe) or args.ffprobe).resolve()
    if ffprobe != ffmpeg.with_name('ffprobe'):
        raise Invalid('current checking requires the installed pinned oracle pair')
    key = (str(ffmpeg), sha256_file(ffmpeg), sha256_file(ffprobe))
    if getattr(args, '_verified_composed_oracle', None) != key:
        bounded([sys.executable, '-I', ROOT / 'scripts/oracle_pin.py', '--verify-bin', ffmpeg.parent], env)
        args._verified_composed_oracle = key


LOADED_METHOD = method_digest()
