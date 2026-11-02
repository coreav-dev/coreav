"""Source-declared Opus presentation and native AAC command expectations."""
from fractions import Fraction
from pathlib import Path
import struct

from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT


PCM_INPUT_FORMAT = 'f32le'


def expected_encoder(rate, channels, bitrate):
    import composed_audio as composed

    composed.integer(rate, 'encoder rate', 1)
    composed.integer(channels, 'encoder channels', 1)
    args = ['-nostdin', '-hide_banner', '-loglevel', 'error', '-f', PCM_INPUT_FORMAT,
            '-ar', str(rate), '-ac', str(channels), '-i', 'pipe:0', '-map', '0:a:0', '-c:a', 'aac']
    if bitrate is not None:
        args += ['-b:a', str(composed.integer(bitrate, 'encoder bitrate', 1))]
    return args + ['-f', 'matroska', 'pipe:1']


def packet_samples(header):
    import composed_audio as composed

    if not header:
        raise composed.Invalid('missing Opus packet header')
    config, code = header[0] >> 3, header[0] & 3
    samples = ((480, 960, 1920, 2880)[config & 3] if config < 12 else
               (480, 960)[config & 1] if config < 16 else (120, 240, 480, 960)[config & 3])
    count = 1 if code == 0 else 2
    if code == 3:
        if len(header) < 2:
            raise composed.Invalid('truncated Opus frame count')
        count = header[1] & 63
    if count == 0 or samples * count > 5760:
        raise composed.Invalid('invalid Opus packet duration')
    return samples * count


def source_window(edits, packets, read):
    import composed_audio as composed

    if not isinstance(packets, list) or not 0 < len(packets) <= 4096:
        raise composed.Invalid('missing or excessive source Opus packets')
    total, spans = 0, []
    for ordinal, packet in enumerate(packets):
        if not isinstance(packet, dict):
            raise composed.Invalid('invalid source Opus packet')
        pts, dts, duration, position, size = (
            composed.decimal(packet.get(key), 'source Opus ' + key)
            for key in ('pts', 'dts', 'duration', 'pos', 'size'))
        if size == 0 or not any(a <= position and position + size <= b for a, b in edits.media_spans):
            raise composed.Invalid('source Opus packet is outside its media bytes')
        samples = packet_samples(read(position, min(size, 2)))
        if pts != total or dts != pts or duration == 0 or duration > samples or \
                (ordinal + 1 < len(packets) and duration != samples):
            raise composed.Invalid('source Opus packet clock is inconsistent')
        total += samples
        spans.append((position, position + size))
    spans.sort()
    if any(a[1] > b[0] for a, b in zip(spans, spans[1:])):
        raise composed.Invalid('source Opus packets overlap')
    full = total - edits.pre_skip
    elapsed, leading = 0, 0
    for duration, media_time, _, _ in edits.entries:
        begin = int(Fraction(elapsed * 48000, edits.movie_scale) + Fraction(1, 2))
        elapsed += duration
        count = int(Fraction(elapsed * 48000, edits.movie_scale) + Fraction(1, 2)) - begin
        if media_time == -1:
            leading = count
        else:
            start = media_time - edits.pre_skip
            end = start + count
    maximum = composed.registration()[0]['capture']['max_pcm_bytes']
    if not 0 <= start < end <= full or not 0 < full * edits.channels * 4 <= maximum or \
            (leading + end - start) * edits.channels * 4 > maximum:
        raise composed.Invalid('source edit window is outside complete decoded media')
    return start, end, leading, full


def source_reference(args, directory, source, index, channels, env):
    import composed_audio as composed

    maximum = composed.registration()[0]['capture']['max_pcm_bytes']
    target = Path(directory) / 'libopus.f32'
    with open(source, 'rb') as data:
        matroska = data.read(4) == bytes.fromhex('1a45dfa3')
    prefix = [] if matroska else ['-ignore_editlist', '1']
    plan = None
    if not matroska:
        raw, _ = composed.bounded([args.ffprobe, '-v', 'error', '-ignore_editlist', '1',
            '-select_streams', str(index), '-show_packets', '-show_streams', '-show_entries',
            'packet=pts,dts,duration,pos,size:stream=index,id,codec_name,sample_rate,channels,time_base',
            '-of', 'json', source], env)
        document = composed.json_document(raw)
        streams = document.get('streams')
        if not isinstance(streams, list) or len(streams) != 1 or not isinstance(streams[0], dict):
            raise composed.Invalid('source Opus stream identity is missing')
        stream = streams[0]
        if stream.get('index') != index or stream.get('codec_name') != 'opus' or \
                stream.get('channels') != channels or stream.get('time_base') != '1/48000' or \
                composed.decimal(stream.get('sample_rate'), 'source Opus rate') != 48000:
            raise composed.Invalid('source Opus stream format differs')
        try:
            track = int(stream['id'], 16)
        except (KeyError, ValueError, TypeError) as error:
            raise composed.Invalid('source Opus track identity is missing') from error
        reader = composed.Headers(source)
        try:
            edits = reader.selected(track, source_opus=True)
            if edits.channels != channels:
                raise composed.Invalid('source Opus channel descriptions differ')
            plan = source_window(edits, document.get('packets'), reader.read)
        finally:
            reader.file.close()
        maximum = plan[3] * channels * 4
    full_path = Path(directory) / 'libopus-full.f32'
    try:
        with full_path.open('xb') as sink:
            composed.bounded([args.ffmpeg, '-nostdin', '-v', 'error', *prefix,
                '-request_sample_fmt', 'flt', '-c:a', 'libopus', *AUDIO_DECODE_INPUT, '-i', source,
                '-map', f'0:{index}', '-c:a', 'pcm_f32le', '-f', PCM_INPUT_FORMAT, 'pipe:1'],
                env, limit=maximum, output=sink)
        size = full_path.stat().st_size
        if size == 0 or size % (channels * 4) or (plan is not None and size != maximum):
            raise composed.Invalid('full libopus decode does not match its source packet work')
        with full_path.open('rb') as data, target.open('xb') as output:
            if plan is None:
                start, end, leading = 0, size // (channels * 4), 0
            else:
                start, end, leading, _ = plan
            remaining = leading * channels * 4
            while remaining:
                count = min(remaining, 65536)
                output.write(bytes(count))
                remaining -= count
            data.seek(start * channels * 4)
            remaining = (end - start) * channels * 4
            while remaining:
                block = data.read(min(remaining, 65536))
                if not block:
                    raise composed.Invalid('source presentation window is truncated')
                output.write(block)
                remaining -= len(block)
    finally:
        full_path.unlink(missing_ok=True)
    return target
