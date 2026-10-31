#!/usr/bin/env python3
"""Checks the decoders of one release build of the coreav command on the
generated clips of test/release-clips.sh (docs/design/release.md, "Dry
run"):

    release-smoke.py BUILD BINARY CLIPS

BUILD is default or full. Outside Windows, where the builds have decoders,
both builds transcode av1.mp4 (through dav1d) and vp9.webm (through libvpx)
to H.264 with the x264 program that COREAV_X264 names; the full build also
transcodes h264.mp4 (CoreAV's own decoder), which the default build refuses.
Each output is probed: one H.264 stream of 320x240 and 10 samples, as the
clip. On every platform the full build decodes aac.m4a with decode-audio,
and the default build refuses it. Strict-safe mode is the caller's choice
(COREAV_STRICT_SAFE). Exit status 0 when every check holds, 1 otherwise.
Python standard library only.
"""
import json
import pathlib
import subprocess
import sys
import tempfile

# The clips and the backend that decodes each.
BACKENDS = {'av1.mp4': 'dav1d', 'vp9.webm': 'libvpx', 'h264.mp4': 'native'}
FULL_ONLY = {'h264.mp4'}
WIDTH, HEIGHT, FRAMES = 320, 240, 10


def run(binary, *args):
    return subprocess.run([binary, *map(str, args)], capture_output=True, text=True, check=False)


def refused(result):
    """Whether the command refused for a missing decoder: its last line on
    standard error is a coreav.error/1 document of kind unsupported_feature."""
    lines = result.stderr.strip().splitlines()
    if result.returncode == 0 or not lines:
        return False
    try:
        return json.loads(lines[-1]).get('kind') == 'unsupported_feature'
    except json.JSONDecodeError:
        return False


def transcoded(binary, clip, output):
    """The problems of one transcode that has to succeed."""
    result = run(binary, 'transcode', '--preset', 'ultrafast', clip, output)
    if result.returncode != 0:
        return [f'{clip.name}: transcode exited {result.returncode}: {result.stderr.strip()}']
    video = json.loads(result.stdout)['video']
    problems = []
    if video['backend'] != BACKENDS[clip.name] or video['frames'] != FRAMES:
        problems.append(f'{clip.name}: decoded by {video["backend"]}, {video["frames"]} frames')
    probe = run(binary, 'probe', output)
    streams = json.loads(probe.stdout)['streams'] if probe.returncode == 0 else []
    shape = [(s['codec'], s['video']['width'], s['video']['height'], s['sample_count']) for s in streams]
    if shape != [('h264', WIDTH, HEIGHT, FRAMES)]:
        problems.append(f'{clip.name}: the output has {shape}')
    return problems


def main():
    build, binary, clips = sys.argv[1], sys.argv[2], pathlib.Path(sys.argv[3])
    if build not in ('default', 'full'):
        sys.exit(f'unknown build {build}')
    problems = []
    with tempfile.TemporaryDirectory() as out:
        out = pathlib.Path(out)
        if sys.platform != 'win32':
            for name in BACKENDS:
                clip = clips / name
                if build == 'full' or name not in FULL_ONLY:
                    problems += transcoded(binary, clip, out / f'{name}.mp4')
                elif not refused(run(binary, 'transcode', clip, out / f'{name}.mp4')):
                    problems.append(f'{name}: the {build} build did not refuse it')
        result = run(binary, 'decode-audio', clips / 'aac.m4a', out / 'aac.wav')
        if build == 'full' and result.returncode != 0:
            problems.append(f'aac.m4a: decode-audio exited {result.returncode}: {result.stderr.strip()}')
        if build == 'default' and not refused(result):
            problems.append(f'aac.m4a: the {build} build did not refuse it')
    for problem in problems:
        print(f'{build} build: {problem}')
    print(f'{build} build: {len(problems)} problems')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
