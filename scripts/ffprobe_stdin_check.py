#!/usr/bin/env python3
"""Check complete structured stdin output and replay on generated inputs."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from input_summary_check import Oracle, CheckFailure as OracleFailure


class CheckFailure(Exception):
    pass


def run(argv, data=None, *, env=None):
    try:
        return subprocess.run(list(map(str, argv)), input=data, capture_output=True,
                              timeout=30, env=env)
    except subprocess.TimeoutExpired:
        raise CheckFailure('child-timeout') from None
    except OSError:
        raise CheckFailure('child-spawn') from None


def check(coreav, oracle, root):
    env = {**os.environ, 'COREAV_STRICT_SAFE': '1', 'TMPDIR': str(root)}
    fallback_env = {**env, 'COREAV_FALLBACK': str(oracle.directory)}
    fallback_env.pop('COREAV_STRICT_SAFE', None)
    args = ['-v', 'error', '-of', 'json', '-show_format', '-show_streams']
    video = ['-f', 'lavfi', '-i', 'testsrc2=s=128x96:r=10:d=0.4']
    sound = ['-f', 'lavfi', '-i', 'sine=frequency=709:sample_rate=48000:duration=0.4']
    codecs = ['-c:v', 'libx264', '-threads:v', '1', '-pix_fmt', 'yuv420p', '-c:a', 'aac']
    cases = []
    for name, suffix, options, native in (
        ('faststart', 'mp4', [*video, *sound, *codecs, '-movflags', '+faststart'], True),
        ('faststart-large', 'mp4', ['-f', 'lavfi', '-i', 'testsrc2=s=320x240:r=30:d=1',
                                  '-c:v', 'libx264', '-threads:v', '1', '-crf', '8',
                                  '-movflags', '+faststart'], True),
        ('tail-moov', 'mp4', [*video, *sound, *codecs], False),
        ('fragmented', 'mp4', [*video, *sound, *codecs,
                             '-movflags', 'frag_keyframe+empty_moov'], False),
        ('matroska', 'mkv', [*video, *sound, *codecs], False),
        ('transport', 'ts', [*video, *sound, *codecs], False),
    ):
        path = root / (name + '.' + suffix)
        made = run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-y', *options, path])
        if made.returncode or not path.is_file():
            raise CheckFailure('fixture-generation')
        if name == 'faststart-large' and path.stat().st_size <= 65536:
            raise CheckFailure('fixture-buffer-contrast')
        cases.append((name, path.read_bytes(), native))
    malformed = bytearray(cases[0][1])
    child = malformed.index(b'moov') + 4
    malformed[child:child + 4] = b'\xff\xff\xff\xff'
    cases.extend([('empty', b'', False), ('truncated', cases[0][1][:64], False),
                  ('malformed-movie', bytes(malformed), False)])
    for name, data, native in cases:
        for spelling in ('pipe:0', 'pipe:'):
            command = [*args, '-i', spelling]
            reference = run([oracle.ffprobe, '-hide_banner', *command], data)
            result = run([coreav, 'ffprobe', *command], data, env=env)
            if native:
                if reference.returncode or reference.stderr or result.returncode or result.stderr \
                        or not result.stdout or result.stdout != reference.stdout:
                    raise CheckFailure('native-' + name)
                parsed = json.loads(result.stdout)
                if parsed['format']['filename'] != spelling or \
                        any(key in parsed['format'] for key in ('size', 'bit_rate')):
                    raise CheckFailure('logical-pipe-facts')
            else:
                if result.returncode != 4 or result.stdout or b'Input #' in result.stderr:
                    raise CheckFailure('refusal-' + name)
                fallback = run([coreav, 'ffprobe', *command], data, env=fallback_env)
                notice, separator, diagnostics = fallback.stderr.partition(b'\n')
                if fallback.returncode != reference.returncode or fallback.stdout != reference.stdout \
                        or not separator or not notice.startswith(b'coreav: ran ') \
                        or diagnostics != reference.stderr:
                    raise CheckFailure('replay-' + name)
        if list(root.glob('coreav-encoded-stdin-*')):
            raise CheckFailure('input-cleanup')
        if native:
            compact = ['-v', 'error', '-of', 'json=compact=1', '-show_format', '-show_streams',
                       '-i', 'pipe:0']
            reference = run([oracle.ffprobe, '-hide_banner', *compact], data)
            result = run([coreav, 'ffprobe', *compact], data, env=env)
            if reference.returncode or reference.stderr or result.returncode or result.stderr \
                    or result.stdout != reference.stdout:
                raise CheckFailure('native-' + name)
            default = [*args[2:], '-i', 'pipe:0']
            reference = run([oracle.ffprobe, '-hide_banner', *default], data)
            result = run([coreav, 'ffprobe', *default], data, env=env)
            if reference.returncode or result.returncode or result.stderr \
                    or result.stdout != reference.stdout:
                raise CheckFailure('native-' + name)
    for options in (
        ['-i', 'pipe:0'],
        [*args, '-show_packets', '-i', 'pipe:0'],
        [*args, '-count_frames', '-i', 'pipe:0'],
        [*args, '-read_intervals', '%+#1', '-i', 'pipe:0'],
        ['-show_streams', '-i', 'pipe:0'],
        [*args, '-i', 'pipe:1'],
        [*args, '-i', '-'],
        ['-v', 'debug', *args[2:], '-i', 'pipe:0'],
    ):
        child = subprocess.Popen([str(coreav), 'ffprobe', *options], stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
        try:
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                raise CheckFailure('shape-before-stdin') from None
            output, diagnostics = child.communicate()
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate()
        if child.returncode != 4 or output or b'Input #' in diagnostics:
            raise CheckFailure('shape-before-stdin')
    bad_temp = root / 'not-a-directory'
    bad_temp.write_bytes(b'preserve')
    failed = run([coreav, 'ffprobe', *args, '-i', 'pipe:0'], cases[0][1],
                 env={**env, 'TMPDIR': str(bad_temp)})
    if failed.returncode == 0 or failed.stdout or bad_temp.read_bytes() != b'preserve':
        raise CheckFailure('temporary-file-failure')
    source = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        failed = subprocess.run([str(coreav), 'ffprobe', *args, '-i', 'pipe:0'], stdin=source,
                                capture_output=True, timeout=30, env=env)
    finally:
        os.close(source)
    if failed.returncode != 3 or failed.stdout \
            or not failed.stderr.startswith(b'coreav ffprobe: could not acquire encoded stdin:') \
            or list(root.glob('coreav-encoded-stdin-*')):
        raise CheckFailure('input-read-failure')
    minus = [*args, '-i', '-']
    reference = run([oracle.ffprobe, '-hide_banner', *minus], cases[0][1])
    fallback = run([coreav, 'ffprobe', *minus], cases[0][1], env=fallback_env)
    if fallback.returncode != reference.returncode or fallback.stdout != reference.stdout \
            or fallback.stderr.partition(b'\n')[2] != reference.stderr:
        raise CheckFailure('fd-alias-fallback')
    local = root / 'faststart.mp4'
    reference = run([oracle.ffprobe, '-hide_banner', *args, '-i', local])
    result = run([coreav, 'ffprobe', *args, '-i', local], env=env)
    if reference.returncode or reference.stderr or result.returncode or result.stderr \
            or result.stdout != reference.stdout:
        raise CheckFailure('local-file-regression')
    relay = root / 'live-fallback'
    relay.mkdir()
    executable = relay / 'ffprobe'
    executable.write_text('#!/usr/bin/env python3\n'
                          'import hashlib, json, sys\n'
                          'print("READY", flush=True)\n'
                          'data = sys.stdin.buffer.read()\n'
                          'print(json.dumps([sys.argv[1:], len(data), hashlib.sha256(data).hexdigest()]), flush=True)\n'
                          'sys.exit(37)\n')
    executable.chmod(0o700)
    for data in (b'not media', b'\x00\x00\x00\x08ftyp\x00\x00\x00\x08moov'
                 + b'\x80\x00\x00\x00mdat' + bytes(1_048_576 - 24)):
        child = subprocess.Popen([str(coreav), 'ffprobe', *args, '-i', 'pipe:0'],
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE,
                                 env={**fallback_env, 'COREAV_FALLBACK': str(relay)})
        try:
            child.stdin.write(data)
            child.stdin.flush()
            with selectors.DefaultSelector() as ready:
                ready.register(child.stdout, selectors.EVENT_READ)
                if not ready.select(5) or child.stdout.readline() != b'READY\n':
                    raise CheckFailure('live-fallback-start')
            tail = b'live remainder\x00\xff'
            output, diagnostics = child.communicate(tail, timeout=5)
            if child.returncode != 37 or json.loads(output) != [
                    ['-hide_banner', *args, '-i', 'pipe:0'], len(data + tail),
                    hashlib.sha256(data + tail).hexdigest()] \
                    or not diagnostics.startswith(b'coreav: ran '):
                raise CheckFailure('live-fallback-replay')
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate()
    executable.write_text('#!/usr/bin/env python3\n'
                          'import sys\n'
                          'sys.exit(29)\n')
    child = subprocess.Popen([str(coreav), 'ffprobe', *args, '-i', 'pipe:0'],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE,
                             env={**fallback_env, 'COREAV_FALLBACK': str(relay)})
    try:
        child.stdin.write(b'not media')
        child.stdin.flush()
        child.wait(timeout=5)
        if child.returncode != 29 or child.stdout.read():
            raise CheckFailure('live-fallback-early-exit')
    finally:
        if child.poll() is None:
            child.kill()
        child.communicate()
    if list(root.glob('coreav-encoded-stdin-*')):
        raise CheckFailure('input-cleanup')
    print('PASS generated structured stdin and complete replay')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle-dir', type=Path, required=True)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    try:
        oracle = Oracle(args.oracle_dir)
        args.directory.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='ffprobe-stdin-', dir=args.directory) as owned:
            check(args.coreav.resolve(), oracle, Path(owned))
    except CheckFailure as error:
        known = {'fixture-generation', 'fixture-buffer-contrast', 'logical-pipe-facts',
                 'input-cleanup', 'shape-before-stdin', 'temporary-file-failure', 'local-file-regression',
                 'input-read-failure', 'fd-alias-fallback', 'child-timeout', 'child-spawn',
                 'live-fallback-start', 'live-fallback-replay', 'live-fallback-early-exit'}
        known.update(category + '-' + name for category in ('native', 'refusal', 'replay')
                     for name in ('faststart', 'faststart-large', 'tail-moov', 'fragmented',
                                  'matroska', 'transport', 'empty', 'truncated', 'malformed-movie'))
        label = str(error)
        print('FAIL ' + (label if label in known else 'structured-stdin-check'), file=sys.stderr)
        return 1
    except OracleFailure:
        print('FAIL oracle-qualification', file=sys.stderr)
        return 1
    except (OSError, ValueError, subprocess.SubprocessError):
        print('FAIL structured-stdin-check', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
