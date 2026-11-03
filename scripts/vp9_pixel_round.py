#!/usr/bin/env python3
"""Capture and replay the registered, merged-source VP9 pixel diagnostic."""
import argparse
import fcntl
import gzip
import hashlib
import json
import math
import os
import pathlib
import re
import resource
import shutil
import signal
import statistics
import subprocess
import sys
import time
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
PLAN_PATH = pathlib.Path('docs/measurements/vp9-pixel-round/plan.json')
RECIPE_PATH = pathlib.Path('docs/measurements/vp9-pixel-round/build.json')
CAPTURE_SCRIPTS = ('vp9_pixel_round.py', 'vp9_pixel_profiles.py', 'vp9_pixel_build.py',
                   'vp9_pixel_ceiling.py', 'vp9_pixel_job.sh',
                   'oracle_pin.py', 'oracle-ffmpeg.env')
sys.path.insert(0, str(HERE))
import vp9_pixel_profiles as profiles


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON key')
        result[key] = value
    return result


def read(path):
    return json.loads(pathlib.Path(path).read_text(), object_pairs_hook=unique)


def write(path, value):
    path = pathlib.Path(path)
    temporary = path.with_suffix(path.suffix + '.part')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def digest(path):
    h = hashlib.sha256()
    with pathlib.Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def plan(root):
    value = read(root / PLAN_PATH)
    require(value['timed_run_requires_merge'] is True and value['held_out'] is False
            and value['changes_existing_bars'] is False and value['frames'] == 120
            and value['unprofiled_rounds'] == 5 and value['warmups'] == 1,
            'registered run boundary differs')
    require(value['arms'] == ['baseline-dist', 'merged-safe-dist', 'merged-auto-dist',
                              'ffmpeg-pinned', 'ffmpeg-symbols'], 'registered arms differ')
    rows = value['inputs']
    require(len(rows) == 6 and len({r['sha256'] for r in rows}) == 6
            and [r['class'] for r in rows] == ['720p', '720p', '1080p', '1080p', '4K', '4K'],
            'registered input roster differs')
    require(all(re.fullmatch(r'[0-9a-f]{64}', r['sha256']) for r in rows), 'invalid input identity')
    return value


def merge_receipt(registration, candidate):
    require(re.fullmatch(r'[0-9a-f]{40}', candidate or ''), 'candidate must be a full commit')
    match = re.fullmatch(r'(coreav-dev/coreav)#([0-9]+)', registration['implementation_pr'])
    require(match is not None, 'invalid implementation pull request')
    repository, number = match.groups()
    request = urllib.request.Request(f'https://api.github.com/repos/{repository}/pulls/{number}',
                                     headers={'Accept': 'application/vnd.github+json',
                                              'User-Agent': 'coreav-vp9-pixel-round'})
    with urllib.request.urlopen(request, timeout=30) as response:
        payload = response.read((2 << 20) + 1)
    require(len(payload) <= 2 << 20, 'oversized merge receipt')
    value = json.loads(payload, object_pairs_hook=unique)
    require(value.get('merged') is True and value.get('state') == 'closed'
            and value.get('merge_commit_sha') == candidate and value.get('merged_at')
            and value.get('number') == int(number)
            and value.get('base', {}).get('ref') == 'main'
            and value.get('base', {}).get('repo', {}).get('full_name') == repository,
            'implementation is unmerged or candidate differs from its merge receipt')
    return dict(repository=repository, number=int(number), merge_commit_sha=candidate,
                merged_at=value['merged_at'])


def order(registration, index, round_number):
    seed = registration['arms'] if index % 2 == 0 else list(reversed(registration['arms']))
    return seed[round_number:] + seed[:round_number]


def environment():
    return {key: os.environ[key] for key in ('PATH', 'HOME', 'TMPDIR') if key in os.environ}


def bounds():
    resource.setrlimit(resource.RLIMIT_AS, (6 << 30, 6 << 30))
    resource.setrlimit(resource.RLIMIT_FSIZE, (2 << 30, 2 << 30))
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def native_command(root, arm, clip, hashed=False):
    command = ['taskset', '-c', '0', str(root / 'bin' / arm), str(clip), '--backend', 'native',
               '--packets', '100000000', '--max-memory-gib', '4']
    return ['/usr/bin/env', 'COREAV_PIXEL_HASHES=1'] + command if hashed else command


def ffmpeg_command(root, arm, clip, loops, hashed=False):
    command = ['taskset', '-c', '0', str(root / 'bin' / arm), '-v', 'error', '-nostdin',
               '-noautorotate', '-c:v', 'vp9', '-threads', '1', '-stream_loop', str(loops - 1),
               '-i', str(clip), '-map', '0:v:0', '-an', '-sn', '-dn', '-fps_mode', 'passthrough',
               '-frames:v', str(120 * loops), '-enc_time_base:v', 'demux']
    return command + (['-f', 'framemd5', '-'] if hashed
                      else ['-nostats', '-progress', 'pipe:1', '-f', 'null', '-'])


def ffmpeg_hashes(data, width, height):
    text = data.decode()
    dimensions = re.findall(r'^#dimensions\s+0:\s+(\d+)x(\d+)\s*$', text, re.M)
    require(dimensions == [(str(width), str(height))], 'oracle dimensions differ')
    rows = [line.split(',') for line in text.splitlines() if line and not line.startswith('#')]
    require(all(len(row) == 6 and re.fullmatch(r'[0-9a-f]{32}', row[-1].strip()) for row in rows),
            'invalid oracle frame hashes')
    return [row[-1].strip() for row in rows]


def work(data, native, expected, hashed=False):
    if native:
        value = json.loads(data, object_pairs_hook=unique)
        require(value['decoder'] == 'coreav vp9, scalar profile 0' and value['codec'] == 'vp9'
                and value['fed'] == 120 and value['cutoff'] is None,
                'native decoder or packet work differs')
        frames = value['frames']
        require(isinstance(frames, list) and len(frames) == expected, 'native frame count differs')
        require(hashed or all(frame is None for frame in frames), 'timed adapter performed extra work')
        return value
    counts = re.findall(rb'^frame=\s*([0-9]+)\s*$', data, re.M)
    require(counts and int(counts[-1]) == expected, 'oracle repeated work differs')
    return None


def counter_available(data):
    values = re.findall(rb'^\s*([0-9][0-9,]*)\s+cycles:u\b', data, re.M)
    require(len(values) == 1 and int(values[0].replace(b',', b'')) > 0, 'user-cycle counter is unavailable')


def host(root, registration):
    require(sys.platform == 'linux', 'capture requires the registered Linux host')
    receipt = read(root / 'instance.json')
    expected = registration['host']
    features = receipt.get('advanced_machine_features') or {}
    require(receipt.get('machine_type') == expected['machine']
            and receipt.get('provisioning') == expected['provisioning']
            and receipt.get('cpu_platform') == expected['cpu_platform']
            and type(features.get('threadsPerCore')) is int
            and features.get('threadsPerCore') == expected['threads_per_core']
            and features.get('turboMode') == expected['turbo_mode']
            and features.get('performanceMonitoringUnit') == expected['pmu'], 'instance receipt differs')
    request = urllib.request.Request('http://metadata.google.internal/computeMetadata/v1/instance/machine-type',
                                     headers={'Metadata-Flavor': 'Google'})
    with urllib.request.urlopen(request, timeout=5) as response:
        require(response.headers.get('Metadata-Flavor') == 'Google', 'unverified metadata service')
        machine = response.read(4096).decode().strip().rsplit('/', 1)[-1]
    require(machine == expected['machine'], 'capture is not on the registered machine class')
    require(pathlib.Path('/sys/devices/system/cpu/smt/active').read_text().strip() == '0'
            and 0 in os.sched_getaffinity(0), 'SMT or affinity differs')
    cpu = pathlib.Path('/proc/cpuinfo').read_text()
    require(re.search(r'^flags\s*:.*\bavx2\b', cpu, re.M), 'AVX2 is unavailable')
    return dict(receipt=receipt, cpu=cpu, loadavg=list(os.getloadavg()))


class Capture:
    def __init__(self, args):
        self.root = pathlib.Path(args.root).resolve()
        self.out = pathlib.Path(args.out).resolve()
        require(not self.out.exists(), 'capture output exists; implicit reruns are forbidden')
        self.out.mkdir(mode=0o700, parents=True)
        write(self.out / 'complete.json', dict(complete=False))
        require(self.root == ROOT.resolve(), 'execute the tooling staged in this capture bundle')
        self.registration = plan(self.root)
        self.receipt = merge_receipt(self.registration, args.candidate)
        self.host = host(self.root, self.registration)
        require(type(args.max_seconds) is int and 1 <= args.max_seconds <= 7200, 'invalid lifetime bound')
        self.builds = read(self.root / 'builds.json')
        recipe = read(self.root / RECIPE_PATH)
        require(self.builds['candidate'] == args.candidate
                and self.builds['baseline'] == self.registration['baseline_commit']
                and self.builds['plan_sha256'] == digest(self.root / PLAN_PATH)
                and self.builds['plan_sha256'] == recipe['reviewed_plan_sha256']
                and self.builds['merge_receipt'] == self.receipt
                and self.builds['recipe_sha256'] == digest(self.root / RECIPE_PATH)
                and self.builds['reviewed_plan_commit'] == recipe['reviewed_plan_commit']
                and self.builds['profile_overrides'] == recipe['native_profile_overrides']
                and self.builds['oracle_pin_sha256'] == digest(self.root / 'scripts/oracle-ffmpeg.env')
                and self.builds['source_deleted_before_profiling'] is True, 'build provenance differs')
        require(set(self.builds['native']) == set(self.registration['profiles']['native_arms']),
                'native source roster differs')
        adapters = set()
        for arm, record in self.builds['native'].items():
            features = ['coreav-decode-oracle/native-vp9']
            if arm == 'merged-safe-dist':
                features.append(self.registration['native_safe_feature'])
            revision = self.registration['baseline_commit'] if arm == 'baseline-dist' else args.candidate
            require(record['commit'] == revision and record['features'] == features
                    and record['profile'] == 'dist', 'native source or features differ')
            adapters.add(record['adapter_sha256'])
        require(len(adapters) == 1, 'native adapters differ')
        require(self.builds['symbol_source']['commit'] == recipe['ffmpeg_source_commit']
                and self.builds['symbol_source']['archive_sha256'] == recipe['ffmpeg_source_archive_sha256']
                and self.builds['symbol_source']['debug_sections'] is False, 'symbol-source binding differs')
        expected = set(self.registration['arms']) | {'ffprobe'}
        require(set(self.builds['binaries']) == expected, 'binary roster differs')
        for name, sha in self.builds['binaries'].items():
            require(digest(self.root / 'bin' / name) == sha, 'binary bytes changed')
        require(set(self.builds['scripts']) == set(CAPTURE_SCRIPTS), 'tooling roster differs')
        for name, sha in self.builds['scripts'].items():
            require(digest(self.root / 'scripts' / name) == sha, 'capture tooling bytes changed')
        listing = read(self.root / 'files/files.json')
        require([row['sha256'] for row in listing] == [row['sha256'] for row in self.registration['inputs']],
                'original input roster differs')
        self.originals = []
        original_receipts = []
        for row in listing:
            require(row['ext'] in ('webm', 'mkv'), 'original container differs')
            path = self.root / 'files' / (row['sha256'] + '.' + row['ext'])
            require(path.resolve().parent == self.root / 'files' and digest(path) == row['sha256'],
                    'original input path or bytes differ')
            self.originals.append(path)
            original_receipts.append(dict(sha256=row['sha256'], size=path.stat().st_size))
        self.lock = (self.root / '.pixel-round.lock').open('w')
        try:
            fcntl.flock(self.lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with (self.root / '.pixel-round-started.json').open('x') as claim:
                json.dump(dict(candidate=args.candidate, plan_sha256=self.builds['plan_sha256']), claim)
        except BaseException:
            self.lock.close()
            raise
        self.deadline = time.monotonic() + args.max_seconds
        self.calls = []
        self.clips = []
        write(self.out / 'header.json', dict(plan=self.registration, builds=self.builds,
                                            host=self.host, merge_receipt=self.receipt,
                                            originals=original_receipts,
                                            max_seconds=args.max_seconds))
        write(self.out / 'complete.json', dict(complete=False))

    def child(self, command, label, profiled=False, input_data=None, source_blind=False):
        remaining = self.deadline - time.monotonic()
        require(remaining > 0, 'capture lifetime exhausted')
        prefix = self.out / f'call-{len(self.calls):04}'
        write(str(prefix) + '.command.json', [str(part) for part in command])
        measured = label['kind'] == 'timed'
        before = resource.getrusage(resource.RUSAGE_CHILDREN) if measured else None
        start = time.monotonic()
        process = subprocess.Popen([str(part) for part in command], stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, env=environment(), preexec_fn=bounds,
                                   stdin=subprocess.PIPE if input_data is not None else subprocess.DEVNULL,
                                   start_new_session=True)
        try:
            stdout, stderr = process.communicate(input=input_data, timeout=min(300, remaining))
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = process.communicate()
            if source_blind and any(profiles.LOCATION.match(line)
                                    for line in (stdout + stderr).decode(errors='replace').splitlines()):
                stdout, stderr = b'', b''
                write(self.out / 'source-location-fault.json', dict(foreign_source_location=True))
            pathlib.Path(str(prefix) + '.stdout').write_bytes(stdout)
            pathlib.Path(str(prefix) + '.stderr').write_bytes(stderr)
            self.calls.append(dict(label, rc=process.returncode, interrupted=True))
            write(self.out / 'calls.json', self.calls)
            raise
        elapsed = time.monotonic() - start
        if source_blind and any(profiles.LOCATION.match(line)
                                for line in (stdout + stderr).decode(errors='replace').splitlines()):
            write(self.out / 'source-location-fault.json', dict(foreign_source_location=True))
            raise ValueError('foreign source location in function-only capture')
        row = dict(label, rc=process.returncode, unexpected_stderr=bool(stderr) and not profiled)
        if measured:
            after = resource.getrusage(resource.RUSAGE_CHILDREN)
            row.update(wall=elapsed, user=after.ru_utime - before.ru_utime,
                       system=after.ru_stime - before.ru_stime)
        pathlib.Path(str(prefix) + '.stdout').write_bytes(stdout)
        pathlib.Path(str(prefix) + '.stderr').write_bytes(stderr)
        self.calls.append(row)
        write(self.out / 'calls.json', self.calls)
        require(process.returncode == 0 and not row['unexpected_stderr'], 'child failed or emitted diagnostics')
        return stdout, stderr, row

    def packets(self, path, index, first=False):
        command = [self.root / 'bin/ffprobe', '-v', 'error', '-select_streams', 'v:0',
                   '-show_packets', '-show_entries', 'packet=pts,dts,duration,size,data_hash',
                   '-show_data_hash', 'sha256', '-of', 'json']
        if first:
            command += ['-read_intervals', '%+#120']
        data, _, _ = self.child(command + [path], dict(kind='packet-binding', input=index, original=first))
        packets = json.loads(data, object_pairs_hook=unique)['packets']
        require(len(packets) == 120 and all(re.fullmatch(r'SHA256:[0-9a-f]{64}', p['data_hash']) for p in packets),
                'prepared packet work differs')
        return packets

    def prepare(self):
        clips = self.root / 'clips'
        clips.mkdir(mode=0o700, exist_ok=False)
        _, counters, _ = self.child(['perf', 'stat', '-e', 'cycles:u', '--', 'true'],
                                     dict(kind='counter-preflight'), True)
        counter_available(counters)
        for index, (row, original) in enumerate(zip(self.registration['inputs'], self.originals)):
            require(digest(original) == row['sha256'], 'original bytes changed before preparation')
            clip = clips / f'input-{index}.webm'
            command = [self.root / 'bin/ffmpeg-pinned', '-v', 'error', '-nostdin', '-noautorotate',
                       '-i', original, '-map', '0:v:0', '-c', 'copy', '-an', '-sn', '-dn',
                       '-frames:v', '120', '-y', clip]
            self.child(command, dict(kind='prepare', input=index))
            source_packets, prepared = self.packets(original, index, True), self.packets(clip, index)
            require([(p['data_hash'], p['size']) for p in source_packets]
                    == [(p['data_hash'], p['size']) for p in prepared], 'packet copy changed coded bytes')
            require(digest(original) == row['sha256'], 'original bytes changed during preparation')
            self.clips.append(dict(path=str(clip), sha256=digest(clip), packets=prepared))
            write(self.out / 'prepared.json', self.clips)
            gold, _, _ = self.child(ffmpeg_command(self.root, 'ffmpeg-pinned', clip, 1, True),
                                     dict(kind='gold', input=index))
            hashes = ffmpeg_hashes(gold, row['width'], row['height'])
            require(len(hashes) == 120, 'oracle frame count differs')
            native_frames = []
            for arm in self.registration['profiles']['native_arms']:
                data, _, _ = self.child(native_command(self.root, arm, clip, True),
                                         dict(kind='pixel-preflight', input=index, arm=arm))
                document = work(data, True, 120, True)
                require([f['md5'] for f in document['frames']] == hashes
                        and all((f['width'], f['height']) == (row['width'], row['height']) for f in document['frames']),
                        'native pixels or dimensions differ')
                native_frames.append(document['frames'])
            require(all(frames == native_frames[0] for frames in native_frames), 'native presentation differs')
            for arm in ('ffmpeg-pinned', 'ffmpeg-symbols'):
                for loops in (1, 2):
                    data, _, _ = self.child(ffmpeg_command(self.root, arm, clip, loops, True),
                                             dict(kind='repeat-preflight', input=index, arm=arm, loops=loops))
                    require(ffmpeg_hashes(data, row['width'], row['height']) == hashes * loops,
                            'oracle repeated pixel sequence differs')
                self.decode(index, arm, 'work-preflight')

    def decode(self, index, arm, kind, round_number=None, profile=False):
        entry = self.clips[index]
        clip = pathlib.Path(entry['path'])
        require(digest(clip) == entry['sha256'], 'prepared bytes changed')
        native = arm in self.registration['profiles']['native_arms']
        loops = 1 if native else self.registration['ffmpeg_loops'][self.registration['inputs'][index]['class']]
        command = native_command(self.root, arm, clip) if native else ffmpeg_command(self.root, arm, clip, loops)
        label = dict(kind=kind, input=index, arm=arm, loops=loops, round=round_number)
        if profile:
            frequency = self.registration['profiles']['native_frequency' if native else 'ffmpeg_frequency']
            command = ['perf', 'record', '-e', 'cycles:u', '-F', str(frequency),
                       '--call-graph', 'dwarf,32768', '-m', '1024', '-o', self.out / 'perf.data', '--'] + command
        data, diagnostics, record = self.child(command, label, profile)
        work(data, native, 120 * loops)
        record['frames'] = 120 * loops
        write(self.out / 'calls.json', self.calls)
        return diagnostics

    def profile(self, index, arm):
        native = arm != 'ffmpeg-symbols'
        diagnostics = self.decode(index, arm, 'profile', profile=True)
        fields = 'comm,pid,tid,time,period,event,ip,sym,dso'
        command = ['perf', 'script', '-F', fields + (',srcline' if native else ''), '-i', self.out / 'perf.data']
        if native:
            command.insert(2, '--inline')
        data, errors, _ = self.child(command, dict(kind='profile-script', input=index, arm=arm), True,
                                      source_blind=not native)
        if native:
            data, _, _ = self.child(['c++filt', '-s', 'rust'], dict(kind='demangle', input=index, arm=arm),
                                    input_data=data)
        stem = self.out / f'profile-{index}-{arm}'
        with gzip.open(str(stem) + '.gz', 'wb') as stream:
            stream.write(data)
        flat, _, _ = self.child(['perf', 'report', '--stdio', '--no-children', '--call-graph', 'none',
                                 '--sort', 'comm,dso,symbol', '--percent-limit', '0', '-i', self.out / 'perf.data'],
                                dict(kind='profile-report', input=index, arm=arm), source_blind=not native)
        pathlib.Path(str(stem) + '.flat').write_bytes(flat)
        write(str(stem) + '.quality.json', dict(record_diagnostics=diagnostics.decode(), script_diagnostics=errors.decode(),
              lost=profiles.lost(diagnostics, errors, flat)))
        (self.out / 'perf.data').unlink()
        (self.out / 'perf.data.old').unlink(missing_ok=True)

    def run(self):
        try:
            self.prepare()
            for index in range(6):
                for arm in self.registration['arms']:
                    self.decode(index, arm, 'warmup')
                for round_number in range(5):
                    for arm in order(self.registration, index, round_number):
                        self.decode(index, arm, 'timed', round_number)
            for index in range(6):
                for arm in self.registration['profiles']['native_arms'] + ['ffmpeg-symbols']:
                    self.profile(index, arm)
            for name, sha in self.builds['binaries'].items():
                require(digest(self.root / 'bin' / name) == sha, 'binary bytes changed during capture')
            for name, sha in self.builds['scripts'].items():
                require(digest(self.root / 'scripts' / name) == sha, 'tooling bytes changed during capture')
            write(self.out / 'complete.json', dict(complete=True))
            try:
                replay(argparse.Namespace(out=self.out, summary=self.out / 'summary.json'))
            except Exception:
                write(self.out / 'complete.json', dict(complete=False))
                raise
        finally:
            shutil.rmtree(self.root / 'clips', ignore_errors=True)
            self.lock.close()


def validate_calls(rows, registration):
    kinds = {'counter-preflight', 'prepare', 'packet-binding', 'gold', 'pixel-preflight', 'repeat-preflight',
             'work-preflight', 'warmup', 'timed', 'profile', 'profile-script', 'profile-report', 'demangle'}
    require(all(row.get('kind') in kinds for row in rows), 'unknown capture work')
    require(not any(row.get('rc') or row.get('interrupted') or row.get('unexpected_stderr') for row in rows),
            'failed invocation in capture')
    timed = [r for r in rows if r['kind'] == 'timed']
    expected = [(i, arm, r) for i in range(6) for r in range(5) for arm in order(registration, i, r)]
    require([(r['input'], r['arm'], r['round']) for r in timed] == expected, 'timing roster or order differs')
    for kind, arms in [('warmup', registration['arms']),
                       ('profile', registration['profiles']['native_arms'] + ['ffmpeg-symbols']),
                       ('pixel-preflight', registration['profiles']['native_arms']),
                       ('work-preflight', ['ffmpeg-pinned', 'ffmpeg-symbols'])]:
        actual = [(r['input'], r['arm']) for r in rows if r['kind'] == kind]
        require(actual == [(i, arm) for i in range(6) for arm in arms], 'missing or duplicate required work')
    repeated = [(r['input'], r['arm'], r['loops']) for r in rows if r['kind'] == 'repeat-preflight']
    require(repeated == [(i, arm, loops) for i in range(6) for arm in ('ffmpeg-pinned', 'ffmpeg-symbols')
                         for loops in (1, 2)], 'repeated oracle preflight roster differs')
    for kind in ('prepare', 'gold'):
        require([r['input'] for r in rows if r['kind'] == kind] == list(range(6)), 'preparation roster differs')
    require([(r['input'], r['original']) for r in rows if r['kind'] == 'packet-binding']
            == [(i, original) for i in range(6) for original in (True, False)], 'packet roster differs')
    require(sum(r['kind'] == 'counter-preflight' for r in rows) == 1, 'counter preflight differs')
    for kind, arms in [('profile-script', registration['profiles']['native_arms'] + ['ffmpeg-symbols']),
                       ('profile-report', registration['profiles']['native_arms'] + ['ffmpeg-symbols']),
                       ('demangle', registration['profiles']['native_arms'])]:
        require([(r['input'], r['arm']) for r in rows if r['kind'] == kind]
                == [(i, arm) for i in range(6) for arm in arms], 'profile evidence roster differs')
    for row in rows:
        if row['kind'] in ('timed', 'warmup', 'profile', 'work-preflight'):
            native = row['arm'] in registration['profiles']['native_arms']
            loops = 1 if native else registration['ffmpeg_loops'][registration['inputs'][row['input']]['class']]
            require(row.get('loops') == loops and row.get('frames') == 120 * loops, 'decoded work differs')
        if row['kind'] == 'timed':
            require(all(type(row.get(k)) in (int, float) and math.isfinite(row[k]) and row[k] > 0
                        for k in ('wall', 'user')) and type(row.get('system')) in (int, float)
                    and math.isfinite(row['system']) and row['system'] >= 0, 'invalid timing value')
    require(next(i for i, r in enumerate(rows) if r['kind'] == 'timed')
            > max(i for i, r in enumerate(rows) if r['kind'] in ('counter-preflight', 'prepare', 'packet-binding',
                     'gold', 'pixel-preflight', 'repeat-preflight', 'work-preflight')),
            'timing preceded pixel or work preflight')
    for index in range(6):
        require(max(i for i, r in enumerate(rows) if r['kind'] == 'warmup' and r['input'] == index)
                < min(i for i, r in enumerate(rows) if r['kind'] == 'timed' and r['input'] == index),
                'timing preceded warmup')
    require(max(i for i, r in enumerate(rows) if r['kind'] == 'timed')
            < min(i for i, r in enumerate(rows) if r['kind'] == 'profile'), 'profiling preceded timing completion')
    return timed


def validate_evidence(directory, rows, registration):
    gold, native_frames, packets, graphs, diagnostics = {}, {}, {}, {}, {}
    for index, row in enumerate(rows):
        prefix = directory / f'call-{index:04}'
        data = pathlib.Path(str(prefix) + '.stdout').read_bytes()
        errors = pathlib.Path(str(prefix) + '.stderr').read_bytes()
        command = read(str(prefix) + '.command.json')
        require(isinstance(command, list) and command and all(isinstance(word, str) for word in command),
                'missing invocation provenance')
        kind = row['kind']
        if kind == 'counter-preflight':
            counter_available(errors)
        if kind not in ('counter-preflight', 'profile', 'profile-script'):
            require(not errors, 'unexpected retained diagnostics')
        if kind in ('timed', 'warmup', 'profile', 'work-preflight'):
            work(data, row['arm'] in registration['profiles']['native_arms'], row['frames'])
        if kind == 'gold':
            entry = registration['inputs'][row['input']]
            gold[row['input']] = ffmpeg_hashes(data, entry['width'], entry['height'])
            require(len(gold[row['input']]) == 120, 'retained oracle frame count differs')
        if kind in ('pixel-preflight', 'repeat-preflight'):
            entry = registration['inputs'][row['input']]
            if kind == 'pixel-preflight':
                frames = work(data, True, 120, True)['frames']
                require([f['md5'] for f in frames] == gold[row['input']]
                        and all((f['width'], f['height']) == (entry['width'], entry['height']) for f in frames),
                        'retained native pixels or dimensions differ')
                previous = native_frames.setdefault(row['input'], frames)
                require(previous == frames, 'retained native presentation differs')
            else:
                require(ffmpeg_hashes(data, entry['width'], entry['height']) == gold[row['input']] * row['loops'],
                        'retained repeated pixels differ')
        if kind == 'packet-binding':
            values = json.loads(data, object_pairs_hook=unique)['packets']
            require(len(values) == 120 and all(re.fullmatch(r'SHA256:[0-9a-f]{64}', p['data_hash']) for p in values),
                    'retained packet work differs')
            coded = [(p['data_hash'], p['size']) for p in values]
            if row['original']:
                packets[row['input']] = coded
            else:
                require(coded == packets[row['input']], 'retained packet copy differs')
        if kind in ('profile', 'profile-script', 'demangle', 'profile-report'):
            key = (row['input'], row['arm'])
            if kind == 'profile':
                diagnostics[key] = errors
            elif kind == 'profile-script':
                diagnostics[key] = (diagnostics[key], errors)
                graphs[key] = data
            elif kind == 'demangle':
                graphs[key] = data
            else:
                stem = directory / f'profile-{key[0]}-{key[1]}'
                require(pathlib.Path(str(stem) + '.flat').read_bytes() == data, 'flat profile bytes differ')
                with gzip.open(str(stem) + '.gz', 'rb') as stream:
                    require(stream.read() == graphs[key], 'profile graph bytes differ')
                record, script = diagnostics[key]
                require(read(str(stem) + '.quality.json') == dict(record_diagnostics=record.decode(),
                        script_diagnostics=script.decode(), lost=profiles.lost(record, script, data)),
                        'profile quality differs')


def replay(args):
    directory = pathlib.Path(args.out)
    write(args.summary, dict(complete=False, diagnostic_only=True, performance_vote=False))
    require(read(directory / 'complete.json') == dict(complete=True), 'capture is incomplete')
    require(not (directory / 'source-location-fault.json').exists(), 'foreign source-location fault')
    header = read(directory / 'header.json')
    registration = plan(ROOT)
    require(header['plan'] == registration
            and header['builds']['plan_sha256'] == digest(ROOT / PLAN_PATH)
            and header['builds']['candidate'] == header['merge_receipt']['merge_commit_sha']
            and header['builds']['baseline'] == registration['baseline_commit']
            and header['builds']['merge_receipt'] == header['merge_receipt']
            and header['builds']['recipe_sha256'] == digest(ROOT / RECIPE_PATH)
            and header['builds']['plan_sha256'] == read(ROOT / RECIPE_PATH)['reviewed_plan_sha256']
            and set(header['builds']['scripts']) == set(CAPTURE_SCRIPTS)
            and all(digest(ROOT / 'scripts' / name) == sha for name, sha in header['builds']['scripts'].items()),
            'replay source binding differs')
    rows = read(directory / 'calls.json')
    timed = validate_calls(rows, registration)
    validate_evidence(directory, rows, registration)
    inputs = []
    for index in range(6):
        calls = [r for r in timed if r['input'] == index]
        medians = {arm: {k: statistics.median(r[k] / r['loops'] for r in calls if r['arm'] == arm)
                         for k in ('user', 'system', 'wall')} for arm in registration['arms']}
        pairs = []
        for round_number in range(5):
            paired = {r['arm']: r for r in calls if r['round'] == round_number}
            pairs.append({metric: dict(total_gain=paired['baseline-dist'][metric] / paired['merged-auto-dist'][metric],
                           scalar_gain=paired['baseline-dist'][metric] / paired['merged-safe-dist'][metric],
                           simd_increment=paired['merged-safe-dist'][metric] / paired['merged-auto-dist'][metric],
                           remaining_ffmpeg_ratio=paired['merged-auto-dist'][metric]
                            / (paired['ffmpeg-pinned'][metric] / paired['ffmpeg-pinned']['loops']))
                          for metric in ('user', 'wall')})
        attributed = {}
        for arm in registration['profiles']['native_arms'] + ['ffmpeg-symbols']:
            stem = directory / f'profile-{index}-{arm}'
            cpu = medians['ffmpeg-pinned' if arm == 'ffmpeg-symbols' else arm]['user']
            attributed[arm] = profiles.analyse(str(stem) + '.gz', arm != 'ffmpeg-symbols', cpu)
            flat = pathlib.Path(str(stem) + '.flat').read_bytes()
            totals = re.findall(rb'^# Event count \(approx.\):\s*([0-9]+)\s*$', flat, re.M)
            require(len(totals) == 1 and int(totals[0]) == attributed[arm]['period'],
                    'exclusive profile periods do not reconcile with the flat report')
            quality = read(str(stem) + '.quality.json')
            attributed[arm]['lost_samples'] = quality['lost']
        inputs.append(dict(input=index, resolution=registration['inputs'][index]['class'],
                           medians=medians, paired=pairs, profiles=attributed,
                           paired_ranges={metric: {name: [min(p[metric][name] for p in pairs),
                                                         max(p[metric][name] for p in pairs)]
                                                   for name in pairs[0][metric]} for metric in ('user', 'wall')},
                           symbol_build_drift=medians['ffmpeg-symbols']['user'] / medians['ffmpeg-pinned']['user']))
    classes = {}
    for resolution in ('720p', '1080p', '4K'):
        selected = [r for r in inputs if r['resolution'] == resolution]
        metrics = {}
        for metric in ('user', 'wall'):
            metrics[metric] = {name: statistics.median(statistics.median(p[metric][name] for p in r['paired'])
                                                      for r in selected)
                               for name in ('total_gain', 'scalar_gain', 'simd_increment', 'remaining_ffmpeg_ratio')}
        low, high = registration['prediction'][resolution]
        classes[resolution] = dict(metrics, frozen_prediction=[low, high],
                                   prediction_matched=low <= metrics['user']['total_gain'] <= high)
    result = dict(complete=True, diagnostic_only=True, performance_vote=False,
                  candidate=header['merge_receipt']['merge_commit_sha'], classes=classes, inputs=inputs,
                  calls=[{k: r[k] for k in ('input', 'arm', 'round', 'loops', 'frames', 'rc', 'wall', 'user', 'system')}
                         for r in timed],
                  caveat='Component times are exclusive cycle-share estimates. The symbol build supplies FFmpeg '
                         'shares scaled to the pinned asset CPU; its drift, loss, missing callers, unknown owners '
                         'and sample counts remain visible. Reference storage work is not equivalent.')
    write(args.summary, result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='mode', required=True)
    capture = commands.add_parser('capture')
    capture.add_argument('--root', type=pathlib.Path, required=True)
    capture.add_argument('--candidate', required=True)
    capture.add_argument('--out', required=True)
    capture.add_argument('--max-seconds', type=int, required=True)
    acquire = commands.add_parser('fetch')
    acquire.add_argument('--candidate', required=True)
    acquire.add_argument('--media', required=True)
    report = commands.add_parser('replay')
    report.add_argument('--out', required=True)
    report.add_argument('--summary', required=True)
    args = parser.parse_args()
    try:
        if args.mode == 'capture':
            Capture(args).run()
        elif args.mode == 'fetch':
            registration = plan(ROOT)
            merge_receipt(registration, args.candidate)
            import vp9_pixel_ceiling
            vp9_pixel_ceiling.fetch(args, wanted=registration['inputs'])
        else:
            replay(args)
    except Exception as error:
        print(f'vp9-pixel-round: {args.mode} incomplete ({type(error).__name__})', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
