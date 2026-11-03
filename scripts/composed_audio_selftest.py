#!/usr/bin/env python3
"""Trusted-fixture integration and semantic mutations of the composed check."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
import composed_audio as composed
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
import transcode_check as checker


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('coreav', 'ffmpeg', 'ffprobe', 'opus-compare', 'fixtures'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    env = checker.child_env(args.ffmpeg) | {'COREAV_X264': 'none'}
    checker.ENV = env
    composed.check_policy()
    composed.verify_comparator(args.opus_compare)
    source = Path(args.fixtures) / 'composed-opus-2.mp4'
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        candidate, plain, baseline = (root / name for name in ('captured.mp4', 'plain.mp4', 'baseline.mp4'))
        directory = root / uuid.uuid4().hex
        command = ['-y', '-i', str(source), '-c:v', 'libx264', '-preset', 'ultrafast',
                   '-c:a', 'aac', '-b:a', '96k', str(candidate)]
        composed.bounded([args.coreav, 'ffmpeg', *command[:-1], plain], env)
        composed.bounded([args.coreav, '--capture-encoder-input', directory, 'ffmpeg', *command], env)
        if composed.sha256_file(candidate) != composed.sha256_file(plain):
            raise composed.Invalid('normal and captured encodes have different output bytes')
        composed.bounded([args.ffmpeg, '-nostdin', '-y', *AUDIO_DECODE_INPUT, '-i',
                          *command[2:-1], baseline], env, messages=True)

        def judge():
            for name in ('libopus.f32', 'captured.s16', 'libopus.s16', 'pcm-reference.mp4'):
                (directory / name).unlink(missing_ok=True)
            return composed.compare(args, directory, source, candidate, 1, baseline, 1, command,
                                    1, 96000, checker.compare_encoded_audio, env)

        original = judge()
        if original.get('ok') is not True:
            raise composed.Invalid('trusted composed fixture failed: ' + str(original.get('reason')))
        print('Positive control: ' + json.dumps({
            'source_presentation': original['source_presentation'],
            'captured_frames': original['capture']['frames'],
            'aac_packets': original['encoder']['packets'],
            'priming': original['presentation']['priming'],
            'opus_quality': original['decoder']['quality'],
            'normal_capture_bytes_equal': True}, sort_keys=True), flush=True)
        pcm = directory / 'stdin.f32'
        receipt_path = directory / 'receipt.json'
        original_pcm, original_output, original_receipt = pcm.read_bytes(), candidate.read_bytes(), receipt_path.read_bytes()

        def restore():
            pcm.write_bytes(original_pcm)
            candidate.write_bytes(original_output)
            receipt_path.write_bytes(original_receipt)

        def rejected(label):
            result = judge()
            if result.get('ok') is not False or not result.get('reason'):
                raise composed.Invalid(label + ' passed')
            print('Rejected ' + label + ': ' + result['reason'], flush=True)

        def matched_mutant(data, bitrate=96000):
            pcm.write_bytes(data)
            mutant = root / 'mutant.mp4'
            composed.bounded([args.ffmpeg, '-nostdin', '-v', 'error', *AUDIO_DECODE_INPUT, '-i', candidate,
                              '-f', 'f32le', '-ar', '48000', '-ac', '2', *AUDIO_DECODE_INPUT, '-i', pcm,
                              '-map', '0:v:0', '-map', '1:a:0', '-c:v', 'copy', '-c:a', 'aac',
                              '-b:a', str(bitrate), '-y', mutant], env)
            shutil.copyfile(mutant, candidate)
            receipt = json.loads(original_receipt)
            receipt['input']['pcm']['sha256'] = composed.sha256_file(pcm)
            receipt['output_sha256'] = composed.sha256_file(candidate)
            receipt_path.write_text(json.dumps(receipt))

        matched_mutant(b'\0' * len(original_pcm))
        rejected('wrong PCM with matching AAC')
        restore()
        at = (len(original_pcm) // 2 // 8) * 8
        matched_mutant(original_pcm[at:] + original_pcm[:at])
        rejected('wrong lag with matching AAC')
        restore()
        matched_mutant(original_pcm, 32000)
        receipt = json.loads(receipt_path.read_bytes())
        for key in ('argv', 'launcher_argv'):
            words = receipt['input'][key]
            position = words.index('-b:a')
            words[position + 1] = '32000'
        receipt_path.write_text(json.dumps(receipt))
        rejected('wrong encoder bitrate')
        restore()

        bad = root / 'failed-encoder'
        bad.write_text('#!/bin/sh\ncase "$*" in *encoder=aac*) echo "Encoder aac [controlled stand-in]"; exit 0;; esac\n/bin/cat >/dev/null\nexit 17\n')
        bad.chmod(0o700)
        failed_capture = root / uuid.uuid4().hex
        failed_output = root / 'failed.mp4'
        failed = subprocess.run([args.coreav, '--capture-encoder-input', str(failed_capture), 'ffmpeg',
                                 *command[:-1], str(failed_output)], env=env | {'COREAV_FFMPEG': str(bad)},
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60, check=False)
        if failed.returncode == 0 or b'17' not in failed.stderr or failed_capture.exists() or failed_output.exists():
            raise composed.Invalid('failed child left successful capture or media state')
        for suffix, words in (
                ('multi.mp4', ['-i', str(source), '-i', str(source), '-c', 'copy']),
                ('frame.png', ['-i', str(source), '-frames:v', '1'])):
            unsupported_capture = root / uuid.uuid4().hex
            unsupported_output = root / suffix
            refused = subprocess.run([args.coreav, '--capture-encoder-input', str(unsupported_capture),
                'ffmpeg', '-y', *words, str(unsupported_output)], env=env,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60, check=False)
            if refused.returncode == 0 or unsupported_capture.exists() or unsupported_output.exists():
                raise composed.Invalid('unsupported capture wrote output or a receipt')
    print('Composed fixture, output identity and wrong-PCM/lag/argv/failure mutations passed')


if __name__ == '__main__':
    main()
