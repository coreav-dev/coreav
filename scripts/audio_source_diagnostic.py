"""Decoder and AAC encoder observations on trusted generated fixtures only."""
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

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT


def run(command, *, data=None, env=None):
    return subprocess.run([str(x) for x in command], input=data, env=env,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          check=True, timeout=120).stdout


def wav_pcm(wav, rate):
    if wav[:4] != b'RIFF' or wav[8:12] != b'WAVE':
        raise ValueError('CoreAV did not write a RIFF WAVE file')
    position = 12
    valid_format = False
    while position + 8 <= len(wav):
        name = wav[position:position + 4]
        size = struct.unpack_from('<I', wav, position + 4)[0]
        position += 8
        payload = wav[position:position + size]
        if len(payload) != size:
            raise ValueError('truncated CoreAV WAV chunk')
        if name == b'fmt ':
            tag, channels, hz, _, align, bits = struct.unpack_from('<HHIIHH', payload)
            subtype = payload[24:40] if tag == 0xfffe else None
            valid_format = (channels, hz, align, bits) == (1, rate, 4, 32) and (
                tag == 3 or subtype == bytes.fromhex('0300000000001000800000aa00389b71'))
        if name == b'data':
            if not valid_format or not size or size % 4:
                raise ValueError('CoreAV WAV is not nonempty mono F32 at the source rate')
            return payload
        position += size + size % 2
    raise ValueError('CoreAV WAV has no data chunk')


def floats(raw):
    values = array.array('f')
    values.frombytes(raw)
    if sys.byteorder != 'little':
        values.byteswap()
    if not values or not all(math.isfinite(v) for v in values):
        raise ValueError('empty or nonfinite PCM')
    return values


def compare_pcm(ours, reference, *, max_lag=2048, window=4096):
    a, b = floats(ours), floats(reference)
    length = min(len(a), len(b))
    if length < 2 * max_lag + window:
        raise ValueError('PCM is too short for the registered lag window')
    noise = sum((x - y) ** 2 for x, y in zip(a, b))
    signal = sum(y * y for y in b[:length])
    start = (length - window) // 2
    reference_window = b[start:start + window]
    energy = sum(x * x for x in reference_window)
    scores = []
    for lag in range(-max_lag, max_lag + 1):
        candidate = a[start + lag:start + lag + window]
        denominator = math.sqrt(energy * sum(x * x for x in candidate))
        score = sum(x * y for x, y in zip(candidate, reference_window)) / denominator if denominator else None
        scores.append({'lag': lag, 'correlation': score})
    peaks = sorted((s for s in scores if s['correlation'] is not None),
                   key=lambda s: (-s['correlation'], abs(s['lag']), s['lag']))[:3]
    return {
        'coreav_samples': len(a), 'ffmpeg_samples': len(b),
        'coreav_sha256': hashlib.sha256(ours).hexdigest(),
        'ffmpeg_sha256': hashlib.sha256(reference).hexdigest(),
        'equal': ours == reference, 'compared_samples': length,
        'max_absolute_difference': max(abs(x - y) for x, y in zip(a, b)),
        'zero_lag_snr_db': 10 * math.log10(signal / noise) if signal and noise else None,
        'reference_silent': signal == 0,
        'lag_window_start': start, 'lag_window_samples': window,
        'maximum_lag': max_lag, 'positive_lag': 'CoreAV samples are delayed',
        'lag_peaks': peaks, 'zero_lag_correlation': scores[max_lag]['correlation'],
    }


def packet_hashes(ffprobe, path):
    result = json.loads(run([ffprobe, '-v', 'error', '-select_streams', 'a:0',
                             '-show_packets', '-show_entries', 'packet=data_hash',
                             '-show_data_hash', 'sha256', '-of', 'json', path]))
    hashes = [packet['data_hash'] for packet in result['packets']]
    if not hashes:
        raise ValueError('encoded output has no audio packets')
    return hashes


def coreav_encode(program, source, output, env):
    run([program, 'ffmpeg', '-i', source, '-c:v', 'libx264', '-c:a', 'aac', output], env=env)


def diagnose(args, source, rate, directory):
    ffmpeg = args.ffmpeg
    ffprobe = ffmpeg.with_name('ffprobe')
    streams = json.loads(run([ffprobe, '-v', 'error', '-select_streams', 'a:0',
                               '-show_entries', 'stream=codec_name,sample_rate,channels',
                               '-of', 'json', source]))['streams']
    if len(streams) != 1 or (int(streams[0]['sample_rate']), streams[0]['channels']) != (rate, 1):
        raise ValueError('generated source does not have its registered rate and mono layout')
    env = dict(os.environ, COREAV_FFMPEG=str(ffmpeg), COREAV_X264='none',
               COREAV_FALLBACK=str(directory / 'no-fallback'))
    env.pop('COREAV_STRICT_SAFE', None)
    wav = directory / 'coreav.wav'
    run([args.coreav, 'decode-audio', '--format', 'f32', '--threads', '1', source, wav], env=env)
    ours = wav_pcm(wav.read_bytes(), rate)
    reference = run([ffmpeg, '-v', 'error', *AUDIO_DECODE_INPUT, '-i', source,
                     '-map', '0:a:0', '-c:a', 'pcm_f32le', '-f', 'f32le', 'pipe:1'])
    result = {'source': source.name, 'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
              'sample_rate': rate, 'channels': 1, 'codec': streams[0]['codec_name'],
              'decoder': compare_pcm(ours, reference)}
    outputs = {}
    for name, pcm in [('coreav_pcm', ours), ('ffmpeg_pcm', reference)]:
        for container in ['matroska', 'mp4']:
            output = directory / f'{name}.{container}'
            raw = [ffmpeg, '-nostdin', '-hide_banner', '-loglevel', 'error', '-f', 'f32le',
                   '-ar', str(rate), '-ac', '1', *AUDIO_DECODE_INPUT, '-i', 'pipe:0',
                   '-map', '0:a:0', '-c:a', 'aac']
            if container == 'matroska':
                output.write_bytes(run([*raw, '-f', 'matroska', 'pipe:1'], data=pcm))
            else:
                run([*raw, '-f', 'mp4', output], data=pcm)
            outputs[f'{name}_{container}'] = packet_hashes(ffprobe, output)
    baseline = directory / 'baseline.mp4'
    run([ffmpeg, '-v', 'error', *AUDIO_DECODE_INPUT, '-i', source,
         '-c:v', 'libx264', '-c:a', 'aac', baseline])
    coreav = directory / 'coreav.mp4'
    coreav_encode(args.coreav, source, coreav, env)
    outputs['baseline'] = packet_hashes(ffprobe, baseline)
    outputs['coreav'] = packet_hashes(ffprobe, coreav)
    result['packet_hashes'] = outputs
    result['encoder'] = {
        'coreav_pcm_reproduces_coreav': outputs['coreav_pcm_matroska'] == outputs['coreav'],
        'ffmpeg_pcm_reproduces_baseline': outputs['ffmpeg_pcm_matroska'] == outputs['baseline'],
        'coreav_pcm_muxers_agree': outputs['coreav_pcm_matroska'] == outputs['coreav_pcm_mp4'],
        'ffmpeg_pcm_muxers_agree': outputs['ffmpeg_pcm_matroska'] == outputs['ffmpeg_pcm_mp4'],
        'coreav_equals_baseline': outputs['coreav'] == outputs['baseline'],
    }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--ffmpeg', type=Path, required=True)
    parser.add_argument('--fixtures', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.coreav, args.ffmpeg = args.coreav.resolve(), args.ffmpeg.resolve()
    run([sys.executable, '-I', HERE / 'oracle_pin.py', '--verify-bin', args.ffmpeg.parent])
    manifest = (args.fixtures / 'MANIFEST').read_text().splitlines()
    if 'generated browser.webm' not in manifest:
        raise ValueError('browser fixture is not registered as generated')
    args.out.mkdir(parents=True, exist_ok=False)
    sources = [(args.fixtures / 'browser.webm', 48_000)]
    for rate in [44_100, 48_000]:
        source = args.out / f'flac-{rate}.mp4'
        run([args.ffmpeg, '-v', 'error', '-f', 'lavfi', '-i',
             'testsrc2=size=320x240:rate=25:duration=2', '-f', 'lavfi', '-i',
             f'sine=frequency=440:duration=2:sample_rate={rate}',
             '-c:v', 'libx264', '-bf', '0', '-pix_fmt', 'yuv420p', '-c:a', 'flac', source])
        sources.append((source, rate))
    report = {'diagnostic_only': True, 'completed': False,
              'coreav_sha256': hashlib.sha256(args.coreav.read_bytes()).hexdigest(),
              'ffmpeg_sha256': hashlib.sha256(args.ffmpeg.read_bytes()).hexdigest(),
              'ffprobe_sha256': hashlib.sha256(args.ffmpeg.with_name('ffprobe').read_bytes()).hexdigest(),
              'ffmpeg_version': run([args.ffmpeg, '-version']).decode().splitlines()[0],
              'results': []}
    report_path = args.out / 'report.json'
    try:
        for source, rate in sources:
            directory = args.out / source.stem
            directory.mkdir()
            report['results'].append(diagnose(args, source, rate, directory))
        report['completed'] = True
    finally:
        report_path.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')


if __name__ == '__main__':
    main()
