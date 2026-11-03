#!/usr/bin/env python3
"""Generate supplemental synthetic V2 inputs with caller-verified oracle programs."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import subprocess
import sys
import tempfile


COMMAND_TIMEOUT = 180
PROBE_TIMEOUT = 30
STDERR_LIMIT = 8000


@dataclass(frozen=True)
class Recipe:
    args: tuple = ()
    length: int | None = None
    text: str | None = None
    dependencies: tuple = ()


def video(duration=2, rate=25):
    return ('-f', 'lavfi', '-i',
            f'testsrc2=size=64x48:rate={rate}:duration={duration}')


def audio(duration):
    return ('-f', 'lavfi', '-i',
            f'sine=frequency=440:sample_rate=48000:duration={duration}')


H264 = ('-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p',
        '-x264-params', 'threads=1', '-g', '25', '-bf', '0')
AAC = ('-c:a', 'aac', '-b:a', '64k', '-ar', '48000', '-ac', '1')
RGBA = video(1, 1) + (
    '-frames:v', '1', '-vf', 'format=rgba', '-c:v', 'png', '-update', '1')
RECIPES = {
    'v2_still_rgba.png': Recipe(RGBA),
    'v2_still_rgba%03d.png': Recipe(RGBA, dependencies=('v2_still_rgba000.png',)),
    'v2_still_rgba000.png': Recipe(RGBA),
    'v2_still_rgb.jpg': Recipe(video(1, 1) + (
        '-frames:v', '1', '-c:v', 'mjpeg', '-pix_fmt', 'yuvj444p', '-update', '1')),
    'v2_audio_mp3_long.mp3': Recipe(audio(480) + (
        '-c:a', 'libmp3lame', '-b:a', '64k', '-ar', '48000', '-ac', '1')),
    'v2_multi_audio_eng.mp4': Recipe(video() + audio(2) + (
        '-map', '0:v:0', '-map', '1:a:0', '-map', '1:a:0',
        '-map', '1:a:0', '-map', '1:a:0') + H264 + AAC + (
        '-metadata:s:a:0', 'language=eng', '-metadata:s:a:1', 'language=und',
        '-metadata:s:a:2', 'language=und', '-metadata:s:a:3', 'language=und')),
    'v2_video_only.mp4': Recipe(video() + H264 + ('-an',)),
    'v2_audio_opus_long.ogg': Recipe(audio(130) + (
        '-c:a', 'libopus', '-b:a', '64k', '-ar', '48000', '-ac', '1')),
    'v2_audio_aac_long.m4a': Recipe(audio(40) + AAC),
    'v2_video_long.mp4': Recipe(video(5027, 1) + H264 + ('-an',)),
    'v2_raw_yuv420p_1280x720.yuv': Recipe(length=1280 * 720 * 3 // 2),
    'v2_raw_yuv420p16le_1280x720.yuv': Recipe(length=1280 * 720 * 3),
    'v2_raw_yuv420p_1920x1080.yuv': Recipe(length=1920 * 1080 * 3 // 2),
    'v2_raw_bgr24_640x480.raw': Recipe(length=640 * 480 * 3),
    'v2_raw_rgb32_800x550.raw': Recipe(length=800 * 550 * 4),
    'v2_raw_yuv420p_400x300_long.yuv': Recipe(length=400 * 300 * 3 // 2 * 25 * 4352),
    'v2_subtitles.srt': Recipe(text='1\n00:00:00,000 --> 00:00:01,000\nSynthetic subtitle.\n'),
    'v2_subtitles_movtext.mp4': Recipe(
        video() + ('-i', 'v2_subtitles.srt', '-map', '0:v:0', '-map', '1:s:0')
        + H264 + ('-c:s', 'mov_text', '-t', '2'), dependencies=('v2_subtitles.srt',)),
    'v2_mpeg_ps_video.mpg': Recipe(video() + (
        '-c:v', 'mpeg2video', '-pix_fmt', 'yuv420p', '-an', '-f', 'mpeg')),
    'v2_video_hevc.mp4': Recipe(video(1, 1) + (
        '-frames:v', '1', '-c:v', 'libx265', '-pix_fmt', 'yuv420p', '-tag:v', 'hvc1',
        '-x265-params', 'pools=none:frame-threads=1:wpp=0:log-level=error', '-an')),
    'v2_hls_local.m3u8': Recipe(
        text='#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n'
             '#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.000,\nv2_hls_segment.ts\n#EXT-X-ENDLIST\n',
        dependencies=('v2_hls_segment.ts',)),
    'v2_hls_segment.ts': Recipe(video() + audio(2) + H264 + AAC + (
        '-map', '0:v:0', '-map', '1:a:0', '-t', '2', '-f', 'mpegts')),
}
FILES = tuple(RECIPES)


class GenerationError(RuntimeError):
    def __init__(self, stage, reason, stderr=''):
        if isinstance(stderr, bytes):
            stderr = stderr.decode('utf-8', errors='replace')
        super().__init__(f'stage={stage} reason={reason}\nstderr={stderr[-STDERR_LIMIT:]}')


def run(stage, command, cwd, timeout):
    try:
        result = subprocess.run(command, cwd=cwd, stdin=subprocess.DEVNULL,
                                capture_output=True, text=True, errors='replace',
                                timeout=timeout, check=False)
    except subprocess.TimeoutExpired as error:
        raise GenerationError(stage, 'timeout', error.stderr or '') from error
    except OSError as error:
        raise GenerationError(stage, 'launch', str(error)) from error
    if result.returncode:
        raise GenerationError(stage, f'exit-{result.returncode}', result.stderr)
    return result.stdout


def selection(only):
    requested = FILES if only is None else tuple(only)
    unknown = set(requested) - RECIPES.keys()
    if unknown:
        raise GenerationError('selection', 'unknown-name', ', '.join(sorted(unknown)))
    selected = []

    def add(name):
        if name in selected:
            return
        for dependency in RECIPES[name].dependencies:
            add(dependency)
        selected.append(name)

    for name in requested:
        add(name)
    return tuple(selected)


def generate(ffmpeg, ffprobe, out, only=None):
    names = selection(only)
    ffmpeg, ffprobe = str(Path(ffmpeg).resolve()), str(Path(ffprobe).resolve())
    out = Path(out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.v2-fixtures-', dir=out) as temporary:
        work = Path(temporary)
        for name in names:
            recipe, target = RECIPES[name], work / name
            try:
                if recipe.length is not None:
                    with target.open('wb') as output:
                        output.truncate(recipe.length)
                elif recipe.text is not None:
                    target.write_text(recipe.text, encoding='utf-8')
                else:
                    run(f'encode:{name}', [
                        ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin', '-y',
                        '-filter_threads', '1', '-filter_complex_threads', '1',
                        *recipe.args, '-threads', '1', '-fflags', '+bitexact',
                        '-flags:v', '+bitexact', '-flags:a', '+bitexact', name,
                    ], work, COMMAND_TIMEOUT)
                    if not target.is_file() or target.stat().st_size == 0:
                        raise GenerationError(f'encode:{name}', 'missing-output')
                if recipe.args or name.endswith('.m3u8'):
                    text = run(f'probe:{name}', [
                        ffprobe, '-v', 'error', '-threads', '1', '-show_entries',
                        'stream=codec_type', '-of', 'json', name,
                    ], work, PROBE_TIMEOUT)
                    try:
                        document = json.loads(text)
                    except ValueError as error:
                        raise GenerationError(f'probe:{name}', 'invalid-json', text) from error
                    if not isinstance(document, dict) or not document.get('streams'):
                        raise GenerationError(f'probe:{name}', 'missing-streams', text)
            except OSError as error:
                raise GenerationError(f'write:{name}', 'io', str(error)) from error
        for name in names:
            try:
                (work / name).replace(out / name)
            except OSError as error:
                raise GenerationError(f'publish:{name}', 'io', str(error)) from error
    return names


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ffmpeg', required=True, type=Path)
    parser.add_argument('--ffprobe', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--only', nargs='+', action='extend', choices=FILES)
    args = parser.parse_args(argv)
    try:
        names = generate(args.ffmpeg, args.ffprobe, args.out, args.only)
    except (GenerationError, OSError) as error:
        if isinstance(error, OSError):
            error = GenerationError('output-directory', 'io', str(error))
        print(error, file=sys.stderr)
        return 1
    for name in names:
        print(f'generated {name}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
