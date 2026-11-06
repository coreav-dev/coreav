"""Reference-only Linux frame path comparison with complete output identities."""
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import statistics
import subprocess
import sys
import time

INPUTS = {
    'input01': ('03be19c3a47b54c5367f6c909e2340037b67f0ad9598cd622d952e2fd3ab5165', '1f25a77f302a8aa8190dc684c0bf8b478b39440b497c629a359e8a028587f5e4', 461774880),
    'input02': ('4b74b8a8a49a683ca0c897fef8e8d3eaa8b9ba94ee7a293f6d657f8d7da25679', '63b3f747a6e07304679c4e018886bc4c708aa2209a1b4069e515c7c72cd286a1', 995328000),
    'input03': ('0a64c0f45511305f32caad62cbac72acdae5cc033ebc56dcb34e80ce584d576e', 'ec3721f8558988a18c4e93fbacdea3083fb82d70914c8772e439a11cda1f0318', 2802470400),
}
WORK, BINS, OUT = map(Path, sys.argv[1:4])
CPUS = str(len(os.sched_getaffinity(0)))
FFMPEG = str(BINS / 'ffmpeg')
TIMEOUT = 240


def identity(path):
    with path.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    return {'bytes': path.stat().st_size, 'sha256': digest}


def call(command, destination=os.devnull):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    with open(destination, 'wb') as output:
        child = subprocess.Popen(command, stdout=output, stderr=subprocess.PIPE, start_new_session=True)
        try:
            _, stderr = child.communicate(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise RuntimeError('reference call timed out') from None
    elapsed = time.perf_counter() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if child.returncode:
        raise RuntimeError('reference call failed')
    return {'seconds': elapsed, **{
        name: getattr(after, field) - getattr(before, field)
        for name, field in [('user_seconds', 'ru_utime'), ('system_seconds', 'ru_stime'),
                            ('minor_faults', 'ru_minflt'), ('major_faults', 'ru_majflt'),
                            ('voluntary_switches', 'ru_nvcsw'), ('involuntary_switches', 'ru_nivcsw')]
    }}


def fetch():
    import boto3
    client = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'],
                          aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'],
                          aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'], region_name='auto')
    wanted = {sha + '.mp4' for sha, *_ in INPUTS.values()}
    index, lists = {}, 0
    for page in client.get_paginator('list_objects_v2').paginate(Bucket=os.environ['R2_BUCKET']):
        lists += 1
        for item in page.get('Contents', []):
            name = item['Key'].rsplit('/', 1)[-1]
            if name in wanted:
                index[name] = item['Key']
    for label, (sha, *_rest) in INPUTS.items():
        original = WORK / 'original.mp4'
        response = client.get_object(Bucket=os.environ['R2_BUCKET'], Key=index[sha + '.mp4'])
        with response['Body'] as source, original.open('wb') as destination:
            while block := source.read(1 << 20):
                destination.write(block)
        if identity(original)['sha256'] != sha:
            raise RuntimeError('input identity mismatch')
        cut = WORK / f'{label}.mp4'
        subprocess.run([FFMPEG, '-nostdin', '-v', 'error', '-y', '-i', str(original), '-map', '0:v:0',
                        '-c', 'copy', '-t', '30', '-fflags', '+bitexact', str(cut)], check=True, timeout=TIMEOUT)
        subprocess.run([FFMPEG, '-nostdin', '-v', 'error', '-y', '-i', str(cut), '-map', '0:v', '-c', 'copy',
                        '-bsf:v', 'h264_mp4toannexb', '-f', 'h264', str(WORK / f'{label}.264')],
                       check=True, timeout=TIMEOUT)
        original.unlink()
    return {'list_requests': lists, 'get_requests': len(INPUTS)}


def arms(label):
    mp4, es = str(WORK / f'{label}.mp4'), str(WORK / f'{label}.264')
    cli = ['--strict-safe', 'ffmpeg', '-i', mp4, '-an', '-pix_fmt', 'yuv420p', '-f', 'rawvideo', '-']
    commands = {}
    for libc in ('musl', 'glibc'):
        for version in ('before', 'after'):
            commands[f'cli_{libc}_{version}'] = [str(BINS / f'coreav-{libc}-{version}')] + cli
    commands['ffmpeg_oracle'] = [FFMPEG, '-nostdin', '-v', 'error', '-noautorotate', '-flags', 'unaligned',
        '-c:v', 'h264', '-threads', CPUS, '-i', mp4, '-an', '-fps_mode', 'passthrough', '-noautoscale',
        '-pix_fmt', 'yuv420p', '-f', 'rawvideo', '-']
    for libc in ('musl', 'glibc'):
        commands[f'decoder_{libc}_write'] = [str(BINS / f'decode-{libc}'), '--threads', CPUS, es]
    return commands


def trace(command, name):
    report = OUT.parent / f'{name}-strace.txt'
    call(['strace', '-c', '-f', '-o', str(report)] + command)
    counts = {}
    for line in report.read_text().splitlines():
        parts = line.split()
        if len(parts) >= 5 and parts[-1] in ('mmap', 'munmap'):
            counts[parts[-1]] = int(parts[3])
    if set(counts) != {'mmap', 'munmap'}:
        raise RuntimeError('incomplete syscall trace')
    return counts


def packet_identity(path):
    probe = subprocess.run([str(BINS / 'ffprobe'), '-v', 'error', '-select_streams', 'v:0', '-show_packets',
        '-show_data_hash', 'sha256', '-show_entries', 'packet=pts,dts,duration,size,flags,data_hash',
        '-of', 'json', str(path)], capture_output=True, check=True, timeout=TIMEOUT)
    packets = json.loads(probe.stdout)['packets']
    if not packets or any(not p.get('data_hash', '').startswith('SHA256:') for p in packets):
        raise RuntimeError('missing encoded packet identities')
    data = json.dumps(packets, sort_keys=True).encode()
    return {'packets': len(packets), 'packet_timeline_sha256': hashlib.sha256(data).hexdigest()}


def transcode():
    rows, expected = [], None
    for round_ in range(3):
        for version in (('before', 'after') if round_ % 2 == 0 else ('after', 'before')):
            destination = WORK / 'encoded.mp4'
            destination.unlink(missing_ok=True)
            command = [str(BINS / f'av1-{version}'), '--strict-safe', 'transcode', '--video', 'av1',
                       '--audio', 'none', '--preset', '8', '--crf', '32', '--timeout', str(TIMEOUT),
                       str(WORK / 'input01.mp4'), str(destination)]
            row = {'arm': version, 'round': round_, **call(command)}
            row['identity'] = packet_identity(destination)
            if expected is None:
                expected = row['identity']
            if row['identity'] != expected:
                raise RuntimeError('transcode packets changed')
            rows.append(row)
    return rows


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    record = {'cpu': next(line.split(':', 1)[1].strip() for line in open('/proc/cpuinfo')
                          if line.startswith('model name')), 'visible_cpus': int(CPUS),
              'build': json.loads((BINS / 'build.json').read_text()), 'r2': fetch(), 'inputs': []}
    for label, (_, raw_sha, raw_bytes) in INPUTS.items():
        commands = arms(label)
        row = {'input': label, 'identity': {}, 'calls': [], 'strace': {}}
        for arm, command in commands.items():
            path = WORK / 'raw.tmp'
            call(command, path)
            row['identity'][arm] = identity(path)
            path.unlink()
            if row['identity'][arm] != {'bytes': raw_bytes, 'sha256': raw_sha}:
                raise RuntimeError('raw output changed')
        names = list(commands)
        for round_ in range(3):
            order = names[round_ % len(names):] + names[:round_ % len(names)]
            if round_ % 2:
                order.reverse()
            for arm in order:
                row['calls'].append({'arm': arm, 'round': round_, **call(commands[arm])})
        for arm, command in commands.items():
            row['strace'][arm] = trace(command, label + '-' + arm)
        record['inputs'].append(row)
        OUT.write_text(json.dumps(record, indent=2) + '\n')
        print(label, 'verified and measured', flush=True)
    record['transcode'] = transcode()
    OUT.write_text(json.dumps(record, indent=2) + '\n')
    lines = ['| Input | Arm | Median seconds | System seconds | Minor faults | mmap | munmap |',
             '|---|---|---:|---:|---:|---:|---:|']
    for row in record['inputs']:
        for arm in arms(row['input']):
            calls = [item for item in row['calls'] if item['arm'] == arm]
            median = lambda key: statistics.median(item[key] for item in calls)
            trace_row = row['strace'][arm]
            lines.append(f"| {row['input']} | {arm} | {median('seconds'):.3f} | {median('system_seconds'):.3f} | "
                         f"{median('minor_faults'):.0f} | {trace_row['mmap']} | {trace_row['munmap']} |")
    print('\n'.join(lines))
    if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(summary, 'a') as stream:
            stream.write('\n'.join(lines) + '\n')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('reference failed:', type(error).__name__, file=sys.stderr)
        sys.exit(1)
