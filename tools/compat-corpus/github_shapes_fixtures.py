"""Generate the registered synthetic media after the main-only run gate."""
import json
import shutil

from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

ORIGINAL = 'github-shapes-200'
PNS_FREE = 'github-shapes-200-pns-free'
REGISTRATIONS = (ORIGINAL, PNS_FREE)


def recipe(registration):
    if registration not in REGISTRATIONS:
        raise ValueError('fixture_registration')
    video = ['-f', 'lavfi', '-i', 'testsrc2=size=64x48:rate=10:duration=2']
    audio = ['-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000:duration=2']
    pns = ['-aac_pns', '0'] if registration == PNS_FREE else []
    return [
        [*video, *audio, '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', *pns, 'source.mp4'],
        ['-i', 'source.mp4', '-c', 'copy', 'source.mkv'],
        [*audio, '-c:a', 'pcm_s16le', 'source.wav'],
        [*audio, '-c:a', 'libmp3lame', 'source.mp3'],
        ['-f', 'lavfi', '-i', 'testsrc=size=64x48:rate=10:duration=2', '-frames:v', '1', 'source.png'],
        [*AUDIO_DECODE_INPUT, '-i', 'source.png', '-frames:v', '1', 'source.jpg'],
        [*video, 'source.gif'],
        [*AUDIO_DECODE_INPUT, '-i', 'source.mp4', '-frames:v', '3', 'frame-%02d.png'],
        [*AUDIO_DECODE_INPUT, '-i', 'source.wav', '-f', 's16le', 'source.s16'],
        [*AUDIO_DECODE_INPUT, '-i', 'source.mp4', '-pix_fmt', 'bgr24', '-f', 'rawvideo', 'source.bgr'],
    ]


def generate(folder, ffmpeg, env, execute, registration):
    commands = recipe(registration)
    folder.mkdir()
    for index, args in enumerate(commands):
        receipt = execute([ffmpeg, '-nostdin', '-v', 'error', '-y', *args], folder, env, 120,
                          f'generate-{index}')
        if receipt['code'] != 0 or receipt['timed_out']:
            raise ValueError('fixture_generation')
    (folder / 'concat.txt').write_text("file 'source.mp4'\nfile 'source.mp4'\n")
    (folder / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')


def stage(command, fixtures, folder):
    for source, _ in command.sources:
        dest = folder / source.split('/')[0]
        dest.mkdir()
        for path in fixtures.iterdir():
            if path.name.startswith(('source.', 'frame-')) or path.name == 'concat.txt':
                shutil.copyfile(path, dest / path.name)
