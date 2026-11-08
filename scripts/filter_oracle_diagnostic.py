import argparse
import collections
import json
import pathlib
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_reference as ref


def diagnose(args):
    args.only = set(args.only.split(','))
    oracle = ref.test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(complete=False, baseline=args.baseline, files=[])
    pool = ref.round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30
    try:
        with tempfile.TemporaryDirectory(prefix='oracle-diagnostic-') as temp:
            for identity, source, digest in ref.inputs(args, oracle, pathlib.Path(temp), pool):
                row = dict(id=identity, source_sha256=digest, variants=[])
                report['files'].append(row)
                raw, _ = ref.command([args.native, source, 'check'], timeout=180)
                native = [json.loads(line) for line in raw.splitlines()]
                pictures = [q for q in native if q.get('type') == 'picture']
                if not pictures or native[-1].get('complete') is not True:
                    raise ValueError('incomplete_native_diagnostic')
                expected = [(q['output_bytes'], q['output_md5']) for q in pictures]
                row['native'] = dict(frames=len(pictures), decoded=native[-1]['decoded_pictures'],
                                     width=pictures[0]['width'], height=pictures[0]['height'],
                                     coded_width=pictures[0].get('coded_width'), coded_height=pictures[0].get('coded_height'),
                                     conformance_window=pictures[0].get('conformance_window'),
                                     depth=pictures[0]['bit_depth'], complete=native[-1]['complete'])
                data = source.read_bytes()
                starts = list(re.finditer(b'\x00\x00(?:\x00)?\x01', data))
                types = collections.Counter()
                first_slices = 0
                for index, start in enumerate(starts):
                    end = starts[index + 1].start() if index + 1 < len(starts) else len(data)
                    nal = data[start.end():end]
                    if len(nal) >= 3:
                        kind = (nal[0] >> 1) & 63
                        types[kind] += 1
                        first_slices += int(kind <= 31 and bool(nal[2] & 128))
                row['prepared'] = dict(bytes=len(data), sha256=ref.conformance.digest(source),
                                       first_slices=first_slices, nal_types=dict(types))
                metadata, _ = ref.command([oracle.ffprobe, '-v', 'error', '-select_streams', 'v:0',
                    '-show_entries', 'stream=pix_fmt,width,height,coded_width,coded_height,sample_aspect_ratio', '-of', 'json', source])
                row['probe'] = json.loads(metadata)['streams'][0]
                pixel_format = row['probe']['pix_fmt']
                if pixel_format not in ({'yuv420p', 'yuvj420p'} if pictures[0]['bit_depth'] == 8 else {'yuv420p10le'}):
                    raise ValueError('oracle_pixel_format')
                try:
                    frames, _ = ref.command([oracle.ffprobe, '-v', 'error', '-flags', 'unaligned',
                        '-select_streams', 'v:0', '-show_frames', '-show_entries', 'frame=width,height,pix_fmt',
                        '-of', 'json', source], timeout=60)
                    row['decoded_probe'] = dict(status='success', frames=json.loads(frames)['frames'])
                except subprocess.TimeoutExpired as error:
                    row['decoded_probe'] = dict(status='timeout', stdout_bytes=len(error.stdout or b''),
                                                stderr_bytes=len(error.stderr or b''))
                for variant in ('output-single-thread', 'rawvideo-single-thread', 'single-filter-thread'):
                    output = ['-pix_fmt', '+' + pixel_format, '-f', 'framemd5', '-']
                    if variant in ('output-single-thread', 'rawvideo-single-thread'):
                        output = ['-threads:v', '1', *output]
                    if variant == 'rawvideo-single-thread':
                        output = ['-c:v', 'rawvideo', *output]
                    command = ref.ffmpeg_command(oracle.ffmpeg, source, True, output)
                    if variant == 'single-filter-thread':
                        command[1:1] = ['-filter_threads', '1']
                    status = 'success'
                    stdout = stderr = b''
                    try:
                        stdout, _ = ref.command(command, timeout=60)
                    except subprocess.TimeoutExpired as error:
                        status, stdout, stderr = 'timeout', error.stdout or b'', error.stderr or b''
                    except subprocess.CalledProcessError as error:
                        status, stdout, stderr = 'exit_failure', error.stdout or b'', error.stderr or b''
                    pairs = []
                    for line in stdout.splitlines():
                        values = line.split(b',')
                        if not line.startswith(b'#') and len(values) == 6:
                            pairs.append((int(values[-2]), values[-1].strip().decode()))
                    dimensions = re.findall(rb'^#dimensions\s+0:\s+(\d+)x(\d+)\s*$', stdout, re.M)
                    row['variants'].append(dict(variant=variant, status=status, stdout_bytes=len(stdout),
                        stderr_bytes=len(stderr), frames=len(pairs), all_hashes_equal=pairs == expected,
                        prefix_hashes_equal=pairs == expected[:len(pairs)], dimensions=[
                            [int(w), int(h)] for w, h in dimensions]))
                    (args.out / 'diagnostic.json').write_text(json.dumps(report, indent=2) + '\n')
        if {row['id'] for row in report['files']} != args.only:
            raise ValueError('incomplete_diagnostic_membership')
        report['complete'] = True
    finally:
        report['r2_requests'] = pool.limits.requests
        pool.close()
        (args.out / 'diagnostic.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--native', type=pathlib.Path, required=True)
    parser.add_argument('--oracle-bin', type=pathlib.Path, required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--baseline', required=True)
    parser.add_argument('--only', required=True)
    parser.add_argument('--frames', type=int, required=True)
    diagnose(parser.parse_args())
