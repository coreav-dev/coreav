import argparse
import json
import pathlib
import resource
import subprocess
import tempfile
import time

import hevc_reference as reference
import hevc_threading as threading
import round_files
import test_oracle

parser = argparse.ArgumentParser()
for name in ('native', 'main', 'oracle-bin', 'out'):
    parser.add_argument('--' + name, type=pathlib.Path, required=True)
args = parser.parse_args()
args.only = {'d14'}
args.frames = 1
args.out.mkdir(parents=True, exist_ok=True)
oracle = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
pool = round_files.pool()
pool.limits.max_requests = 4000
pool.limits.max_bytes = 32 << 30
report = dict(scope='temporary scheduler phase instrumentation, not a speed verdict', files=[])
try:
    with tempfile.TemporaryDirectory() as temp:
        for identity, source, sha in reference.inputs(args, oracle, pathlib.Path(temp), pool):
            row = dict(id=identity, source_sha256=sha)
            report['files'].append(row)
            if identity == 'd14':
                for arm, binary in (('candidate', args.native), ('main', args.main)):
                    stdout, _ = reference.command([binary, source, 'check'], timeout=600)
                    rows = [json.loads(line) for line in stdout.splitlines()]
                    row[arm] = rows
                stdout, _ = reference.command([oracle.ffprobe, '-v', 'error', '-select_streams', 'v:0',
                    '-show_entries', 'stream=pix_fmt,width,height', '-of', 'json', source], timeout=600)
                row['probe'] = json.loads(stdout)
                stdout, _ = reference.command(reference.ffmpeg_command(oracle.ffmpeg, source, True,
                    ['-pix_fmt', '+yuv420p', '-f', 'framemd5', '-']), timeout=600)
                row['oracle_frames'] = stdout.decode()
                row['status'] = 'geometry_diagnosis_only'
                continue
            row.update(reference.verify(args.native, oracle, source, timeout=600))
            expected = threading.signature(args.native, source, 1)
            row['calls'] = []
            for threads in (1, 4, 8):
                if threading.signature(args.native, source, threads) != expected:
                    raise ValueError('profile_thread_output_mismatch')
                before = resource.getrusage(resource.RUSAGE_CHILDREN)
                start = time.monotonic()
                child = subprocess.run([str(args.native), str(source), 'decode', str(threads)],
                    capture_output=True, timeout=600, check=True, env=reference.CHILD_ENV)
                wall = time.monotonic() - start
                after = resource.getrusage(resource.RUSAGE_CHILDREN)
                reference.check_work('coreav', child.stdout, row['frames'])
                row['calls'].append(dict(threads=threads, wall=wall, user=after.ru_utime-before.ru_utime,
                    system=after.ru_stime-before.ru_stime, minor_faults=after.ru_minflt-before.ru_minflt,
                    major_faults=after.ru_majflt-before.ru_majflt, summary=json.loads(child.stdout),
                    phases=[line for line in child.stderr.decode().splitlines() if line.startswith('wpp-profile-')]))
            row['status'] = 'profiled'
finally:
    pool.close()
    report['r2_requests'] = pool.limits.requests
    (args.out / 'profile.json').write_text(json.dumps(report, indent=2) + '\n')
