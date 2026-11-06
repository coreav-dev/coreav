#!/usr/bin/env python3
"""Build the registered comparator and trusted composed-audio fixtures."""
import argparse
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import composed_audio as composed
import test_oracle


def build_comparator(directory, env):
    pin = composed.registration()[0]['opus_compare']
    directory.mkdir()
    program = directory / 'opus_compare'
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        archive = temporary / 'opus.tar.gz'
        composed.bounded(['curl', '--fail', '--silent', '--show-error', '--location',
                          '--proto', '=https', '--proto-redir', '=https', '--connect-timeout', '15',
                          '--max-time', '120', '--max-filesize', str(10 << 20), '-o', archive, pin['url']],
                         env, timeout=125)
        if composed.sha256_file(archive) != pin['sha256']:
            raise composed.Invalid('pinned comparator archive digest differs')
        license_path = 'opus-1.6.1/COPYING'
        composed.bounded(['tar', '-xzf', archive, '-C', temporary, pin['source'], license_path], env)
        composed.bounded(['cc', '-O2', '-o', program, temporary / pin['source'], '-lm'], env, timeout=120)
        shutil.copyfile(temporary / license_path, directory / 'COPYING')
    metadata = {
        'archive_sha256': pin['sha256'],
        'program_sha256': composed.sha256_file(program),
        'builder_sha256': composed.sha256_file(__file__),
        'registration': composed.registration()[1],
    }
    (directory / 'comparator.json').write_text(json.dumps(metadata, sort_keys=True))
    return metadata['program_sha256']


def signal_expression(channels, variant=0):
    left = ('if(lt(t,0.19),0.13*sin(2*PI*(311*t+137*t*t)),'
            'if(between(t,0.47,1.13),0.09*sin(2*PI*(1703*t+91*t*t)),0))')
    right = ('if(between(t,0.23,0.61),0.12*sin(2*PI*(503*t+97*t*t)),'
             'if(gt(t,1.37),0.07*sin(2*PI*(2309*t+173*t*t)),0))')
    if variant:
        left, right = right, left
    return left if channels == 1 else left + '|' + right


def generate(ffmpeg, directory, env):
    for container in ('mp4', 'webm'):
        for channels in (1, 2):
            name = f'composed-opus-{channels}.{container}'
            video = (['-c:v', 'libx264', '-preset', 'ultrafast', '-bf', '0'] if container == 'mp4' else
                     ['-c:v', 'libvpx-vp9', '-deadline', 'realtime', '-cpu-used', '8', '-b:v', '0', '-crf', '38'])
            composed.bounded([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                              'testsrc2=size=96x64:rate=25:duration=2', '-f', 'lavfi', '-i',
                              f"aevalsrc='{signal_expression(channels)}':s=48000:d=2",
                              '-map', '0:v:0', '-map', '1:a:0', *video, '-c:a', 'libopus',
                              '-b:a', '96k', '-shortest', '-n', directory / name], env)
    audio = [f"aevalsrc='{signal_expression(1, variant)}':s=48000:d=2" for variant in (0, 1)]
    composed.bounded([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                      'testsrc2=size=96x64:rate=25:duration=2', '-f', 'lavfi', '-i', audio[0],
                      '-f', 'lavfi', '-i', audio[1], '-map', '0:v:0', '-map', '1:a:0', '-map', '2:a:0',
                      '-c:v', 'libx264', '-preset', 'ultrafast', '-bf', '0', '-c:a', 'libopus',
                      '-b:a', '96k', '-shortest', '-n', directory / 'composed-mapped-opus.mp4'], env)
    for rate in (44100, 48000):
        composed.bounded([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                          'testsrc2=size=96x64:rate=25:duration=2', '-f', 'lavfi', '-i',
                          f"aevalsrc='{signal_expression(1)}':s={rate}:d=2", '-map', '0:v:0', '-map', '1:a:0',
                          '-c:v', 'libx264', '-preset', 'ultrafast', '-bf', '0', '-c:a', 'flac',
                          '-strict', '-2', '-shortest', '-n', directory / f'composed-flac-{rate}.mp4'], env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ffmpeg', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    env = {key: value for key, value in os.environ.items()
           if key != 'CAPY_API_KEY' and not key.startswith(('R2_', 'AWS_', 'COREAV_GCP_', 'CORPUS_R2'))}
    composed.check_policy()
    pair = test_oracle.require(args.ffmpeg)
    args.out.mkdir()
    build_comparator(args.out / 'tools', env)
    generate(pair.ffmpeg, args.out, env)


if __name__ == '__main__':
    main()
