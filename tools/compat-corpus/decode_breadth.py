#!/usr/bin/env python3
"""V1, decode breadth (docs/measurements/compat-0.1.0/codec-coverage-plan.md,
"V1, decode breadth"; docs/design/compat-cli.md, "The 0.1.0 codec coverage
bars").

    decode_breadth.py run --decode-oracle PATH --ffmpeg PATH --ffprobe PATH
                      --files FILES.json --records OUT.jsonl [--shard I/N]
                      [--fetch DIR] [--deadline EPOCH] [--limit-minutes 150]
                      [--rerun-host] [--jobs N]
        Per file of FILES.json (a list of {sha256, key, ext} and either a
        path, or with --fetch the fields of round_files.py select): the
        pinned ffprobe lists the streams and the default video stream is
        chosen (scripts/thumbnail_oracle.py), the pinned ffmpeg prints the
        framemd5 of the whole stream (the reference), and `decode-oracle
        stream` decodes it with CoreAV's backends; the two sides of a file
        run at the same time, each with its own time limit, in its own
        process group and with a clean environment (PATH, HOME, an empty
        GCONV_PATH, and COREAV_STRICT_SAFE=1 for CoreAV's side). The code
        and sub-code of a file that does not count come from the plan's
        ordered tables and decode_reasons.json. One line is appended to
        OUT.jsonl as soon as a file is decided, and a file that already has
        a line is not run again. --shard takes FILES[I::N]. --fetch fetches
        each file just before it is run (round_files.fetch_one) and deletes
        it after its line is written, so at most --jobs files are on disk.
        --deadline: from that time no file is started, and a side still
        running is killed and recorded as host_limit job_end. A side the
        runner stops at its limit, a side killed by SIGKILL it did not send
        (the memory limit), a lack of disk and a failed fetch are host
        limits, recorded as host_limit with no code. With --rerun-host (the
        rerun host) a side stopped at its limit is final instead:
        reference_unfinished or timeout.

    decode_breadth.py pending --records A.jsonl... --files FILES.json
                              [--shard I/N] --out RERUN.json
        The entries of FILES.json (of the shard) with a host limit in their
        line or no line, in the form of FILES.json, for the rerun host.

    decode_breadth.py summarize --records A.jsonl... [--rerun B.jsonl...]
                                --files FILES.json --summary OUT.json
        The plan's result, as counts only: the share, per codec group the
        denominator, counted files and share, misses by code and sub-code,
        the files without a reference, the frames compared and the number
        of files rerun. A line of --rerun replaces the line of the same
        file. Exit 0 when complete, 2 when void: a file of FILES.json with
        no final result, a line that names the FFmpeg fallback as the
        backend, or a line of a file outside FILES.json.

    decode_breadth.py mismatches --records A.jsonl... [--rerun B.jsonl...]
                                 --out FILE.md
        The files that miss as frame_count or md5, with their hash, key,
        group, counts and first differing frame: the private list.

ffmpeg, ffprobe and decode-oracle are black boxes. Python 3 standard library
only.
"""
import argparse
import collections
import concurrent.futures
import errno
import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / 'scripts'))
import thumbnail_oracle  # noqa: E402

BAR = 74
GROUPS = ('h264', 'av1', 'vp9', 'vp8', 'other')
REFERENCE_BACKEND = {'av1': 'dav1d', 'vp9': 'libvpx', 'vp8': 'libvpx'}
NO_REFERENCE = ('no_stream', 'reference_failed', 'reference_empty', 'reference_unfinished')
PROBE_ENTRIES = ('stream=index,codec_type,codec_name,width,height,pix_fmt,field_order,bits_per_raw_sample'
                 ':stream_disposition=attached_pic:format=format_name')
CONTAINERS = ('mov,mp4,m4a,3gp,3g2,mj2', 'matroska,webm')
REJECTED = ('truncated_structure', 'invalid_structure')
KIND_SUBS = {'limit': 'limit', 'invalid_data': 'invalid_data', 'backend': 'backend'}
OUT_OF_DISK = b'No space left on device'


def reasons():
    """The exact-match table, from (stage, kind, message) to (code, sub)."""
    rows = json.loads((HERE / 'decode_reasons.json').read_text())['rows']
    return {(r['stage'], r['kind'], r['message']): (r['code'], r['sub']) for r in rows}


def clean_env(gconv, strict_safe=False):
    env = {'PATH': os.environ.get('PATH', ''), 'HOME': os.environ.get('HOME', ''), 'GCONV_PATH': gconv}
    if strict_safe:
        env['COREAV_STRICT_SAFE'] = '1'
    return env


def run_side(argv, env, limit, job_deadline):
    """Runs argv in its own process group, killing the group at the earlier
    of limit seconds and job_deadline (an epoch time). Records the
    deadline it enforced, the exit status and the signal."""
    end, ended_by = time.time() + limit, 'limit'
    if job_deadline is not None and job_deadline < end:
        end, ended_by = job_deadline, 'job_end'
    side = {'limit_s': limit, 'killed': None, 'exit': None, 'signal': None, 'out': b'', 'err': b'',
            'spawn_errno': None}
    try:
        process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, env=env, start_new_session=True)
    except OSError as e:
        side['spawn_errno'] = e.errno
        return side
    try:
        side['out'], side['err'] = process.communicate(timeout=max(0.0, end - time.time()))
    except subprocess.TimeoutExpired:
        side['killed'] = ended_by
        stop_group(process.pid)
        try:
            side['out'], side['err'] = process.communicate(timeout=60)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    else:
        stop_group(process.pid)
    code = process.returncode
    side['exit'], side['signal'] = (code, None) if code >= 0 else (None, -code)
    return side


def stop_group(pgid):
    try:
        os.killpg(pgid, signal.SIGKILL)
    except OSError:
        pass


def host_limit(side, final):
    """The host limit a finished side shows, or None: from what the runner
    did and saw of the process, never from its result."""
    if side['killed'] == 'job_end':
        return 'job_end'
    if side['killed'] and not final:
        return 'deadline'
    if side['signal'] == signal.SIGKILL and not side['killed']:
        return 'oom'
    if side['spawn_errno'] == errno.ENOSPC or OUT_OF_DISK in side['err'][-4096:]:
        return 'enospc'
    return None


def block(side):
    return {k: side[k] for k in ('limit_s', 'killed', 'exit', 'signal')}


def probe(opts, env, path):
    """The side of the ffprobe run, and the streams and format it lists."""
    side = run_side([opts.ffprobe, '-v', 'error', '-show_entries', PROBE_ENTRIES, '-of', 'json', str(path)],
                    env, opts.limit, opts.deadline)
    listed = None
    if side['exit'] == 0:
        try:
            listed = json.loads(side['out'] or '{}')
        except ValueError:
            listed = None
    return side, listed


def stream_of(listed):
    """The default video stream among the listed ones, and the file's
    format name."""
    if not isinstance(listed, dict):
        return None, None
    streams = listed.get('streams') or []
    index = thumbnail_oracle.choose_default_video(streams)
    stream = next((s for s in streams if s.get('index') == index), None) if index is not None else None
    return stream, (listed.get('format') or {}).get('format_name')


def md5s(out):
    return [line.rsplit(',', 1)[-1].strip() for line in out.decode(errors='replace').splitlines()
            if line and not line.startswith('#')]


def reference(opts, env, path, entry, stream, group):
    command = thumbnail_oracle.framemd5_command(opts.ffmpeg, path, entry['ext'].lower(), stream['index'], None,
                                                REFERENCE_BACKEND.get(group, 'ffmpeg'), stream.get('codec_name'),
                                                stream.get('pix_fmt'), unconverted=True)
    side = run_side(command, env, opts.limit, opts.deadline)
    side['md5'] = md5s(side['out']) if side['exit'] == 0 else []
    return side


def coreav(opts, env, path):
    side = run_side([opts.decode_oracle, 'stream', str(path)], env, opts.limit, opts.deadline)
    obj = None
    if side['exit'] in (0, 1):
        try:
            obj = json.loads(side['out'])
        except ValueError:
            obj = None
    side['object'] = obj if isinstance(obj, dict) and 'outcome' in obj else None
    return side


def first_difference(ours, theirs):
    for i, (a, b) in enumerate(zip(ours, theirs)):
        if a != b:
            return i
    return min(len(ours), len(theirs))


def h264_scope(stream):
    """The sub-code of an H.264 stream that CoreAV reports as unsupported,
    from the properties ffprobe lists: the first that holds."""
    pix_fmt = stream.get('pix_fmt') or ''
    depth = re.search(r'\d+', str(stream.get('bits_per_raw_sample') or ''))
    suffix = re.search(r'(?:p|gray)(\d+)(?:le|be)$', pix_fmt)
    if depth and int(depth.group()) > 8 or suffix and int(suffix.group(1)) > 8:
        return '10bit'
    if pix_fmt and not re.match(r'(yuvj?420p|yuva420p|nv12|nv21|p0\d\d)', pix_fmt):
        return 'chroma'
    if stream.get('field_order') not in (None, 'progressive', 'unknown'):
        return 'interlaced'
    return 'other'


def decide(group, stream, format_name, ours_side, ref_md5, table, final):
    """The record's code, sub-code, text kept with an 'other' and counts of
    a file in the denominator."""
    obj = ours_side['object']
    if obj is None:
        code, sub = ('timeout', None) if ours_side['killed'] and final else ('decode_error', 'crash')
        return {'counted': False, 'code': code, 'sub': sub}
    outcome = obj['outcome']
    ours = [f['md5'] for f in obj.get('frames', [])]
    if (ours_side['exit'] == 0) != (outcome == 'ok'):
        return {'counted': False, 'code': 'decode_error', 'sub': 'crash'}
    if outcome == 'ok' and obj['stream_index'] == stream['index'] and ours == ref_md5:
        return {'counted': True, 'code': None, 'sub': None}
    if outcome != 'ok' and outcome['stage'] == 'read':
        sub = ('container' if format_name not in CONTAINERS else
               'rejected' if outcome['kind'] in REJECTED else
               table.get(('read', outcome['kind'], outcome['message']), (None, 'other'))[1])
        return {'counted': False, 'code': 'not_read', 'sub': sub, 'text': outcome['message']}
    if obj['stream_index'] != stream['index']:
        return {'counted': False, 'code': 'other_stream', 'sub': None}
    if group == 'other':
        return {'counted': False, 'code': 'codec_not_in_build', 'sub': stream.get('codec_name')}
    if outcome != 'ok':
        stage, kind, message = outcome['stage'], outcome['kind'], outcome['message']
        known = table.get((stage, kind, message))
        if kind == 'unsupported':
            sub = h264_scope(stream) if group == 'h264' else (known[1] if known else 'other')
            return {'counted': False, 'code': 'out_of_scope', 'sub': sub, 'text': message}
        sub = known[1] if known and known[0] == 'decode_error' else KIND_SUBS.get(kind, 'other')
        return {'counted': False, 'code': 'decode_error', 'sub': sub, 'text': message}
    if len(ours) != len(ref_md5):
        return {'counted': False, 'code': 'frame_count', 'sub': None}
    return {'counted': False, 'code': 'md5', 'sub': None}


def examine(opts, entry, path, gconv, table):
    """The record of one file."""
    record = {'sha256': entry['sha256'], 'key': entry.get('key'), 'ext': entry['ext'], 'host_limit': None,
              'host_limit_side': None, 'final_deadline': opts.rerun_host, 'group': None, 'codec': None,
              'stream_index': None, 'counted': False, 'code': None, 'sub': None}

    def limited(side, name, final=opts.rerun_host):
        event = host_limit(side, final)
        if event and not record['host_limit']:
            record['host_limit'], record['host_limit_side'] = event, name
        return event

    probed, listed = probe(opts, clean_env(gconv), path)
    record['probe'] = block(probed)
    if limited(probed, 'probe', final=False):
        return record
    stream, format_name = stream_of(listed)
    record['format_name'] = format_name
    if stream is None:
        record['code'] = 'no_stream'
        return record
    codec = stream.get('codec_name')
    group = codec if codec in GROUPS[:4] else 'other'
    record.update(group=group, codec=codec, stream_index=stream['index'], properties={
        k: stream.get(k) for k in ('width', 'height', 'pix_fmt', 'field_order', 'bits_per_raw_sample')})
    with concurrent.futures.ThreadPoolExecutor(2) as sides:
        pending = sides.submit(reference, opts, clean_env(gconv), path, entry, stream, group)
        ours = sides.submit(coreav, opts, clean_env(gconv, strict_safe=True), path)
        ref_side, ours_side = pending.result(), ours.result()
    record['reference'] = {**block(ref_side), 'frames': len(ref_side['md5'])}
    obj = ours_side['object']
    record['coreav'] = {**block(ours_side), 'outcome': obj['outcome'] if obj else None,
                        'stream_index': obj['stream_index'] if obj else None,
                        'backend': obj['backend'] if obj else None, 'decoder': obj['decoder'] if obj else None,
                        'frames': len(obj['frames']) if obj else None}
    limited(ref_side, 'reference')
    limited(ours_side, 'coreav')
    if record['host_limit']:
        return record
    if ref_side['killed']:
        record['code'] = 'reference_unfinished'
        return record
    if ref_side['exit'] != 0:
        record['code'] = 'reference_failed'
        return record
    if not ref_side['md5']:
        record['code'] = 'reference_empty'
        return record
    decision = decide(group, stream, format_name, ours_side, ref_side['md5'], table, opts.rerun_host)
    record.update(decision)
    if obj and obj['outcome'] == 'ok' and obj['stream_index'] == stream['index']:
        theirs = [f['md5'] for f in obj['frames']]
        record['frames'] = {'reference': len(ref_side['md5']), 'coreav': len(theirs),
                            'first_differing': first_difference(theirs, ref_side['md5'])
                            if theirs != ref_side['md5'] else None}
    return record


class Records:
    """The records file, appended one line at a time."""

    def __init__(self, path):
        self.path = pathlib.Path(path)
        self.lock = threading.Lock()
        self.path.touch()
        kept = self.path.read_bytes()
        self.path.write_bytes(kept[:kept.rfind(b'\n') + 1])
        self.done = {r['sha256'] for r in load([path])}

    def append(self, record):
        line = json.dumps(record, sort_keys=True) + '\n'
        with self.lock, open(self.path, 'a') as f:
            f.write(line)
            f.flush()
            os.fsync(f.fileno())
            self.done.add(record['sha256'])


def load(paths):
    out = []
    for path in paths:
        with open(path) as f:
            out.extend(json.loads(line) for line in f if line.strip())
    return out


def shard(files, spec):
    if not spec:
        return files
    i, n = (int(x) for x in spec.split('/'))
    return files[i::n]


def fetch_failure(entry, error):
    event = 'enospc' if isinstance(error, OSError) and error.errno == errno.ENOSPC else 'fetch'
    return {'sha256': entry['sha256'], 'key': entry.get('key'), 'ext': entry['ext'], 'host_limit': event,
            'host_limit_side': 'fetch', 'group': None, 'code': None, 'counted': False}


def run(opts, fetcher=None):
    """Runs the files of opts.files that opts.records does not hold yet.
    fetcher(entry, directory) returns the path of a fetched entry."""
    files = shard(json.loads(pathlib.Path(opts.files).read_text()), opts.shard)
    for program in (opts.decode_oracle, opts.ffmpeg, opts.ffprobe):
        if not os.access(program, os.X_OK):
            raise SystemExit(f'{program} is not an executable')
    records = Records(opts.records)
    table = reasons()
    gconv = tempfile.mkdtemp(prefix='gconv-')
    opts.limit = opts.limit_minutes * 60
    if opts.fetch:
        pathlib.Path(opts.fetch).mkdir(parents=True, exist_ok=True)

    def one(entry):
        if entry['sha256'] in records.done or opts.deadline and time.time() >= opts.deadline:
            return False
        path = None
        try:
            if opts.fetch:
                try:
                    path = fetcher(entry, pathlib.Path(opts.fetch))
                except (Exception, SystemExit) as error:
                    records.append(fetch_failure(entry, error))
                    return True
            records.append(examine(opts, entry, path or pathlib.Path(entry['path']), gconv, table))
        finally:
            if opts.fetch:
                remove_fetched(entry, pathlib.Path(opts.fetch))
        return True

    try:
        with concurrent.futures.ThreadPoolExecutor(max(1, opts.jobs)) as workers:
            ran = sum(workers.map(one, files))
    finally:
        shutil.rmtree(gconv, ignore_errors=True)
    print(json.dumps({'files': len(files), 'run': ran}))
    return 0


def remove_fetched(entry, directory):
    for name in (f"{entry['sha256']}.{entry['ext']}", f"{entry['sha256']}.{entry['ext']}.part"):
        (directory / name).unlink(missing_ok=True)


def pending(opts):
    held = {r['sha256']: r for r in load(opts.records)}
    files = [e for e in shard(json.loads(pathlib.Path(opts.files).read_text()), opts.shard)
             if e['sha256'] not in held or held[e['sha256']].get('host_limit')]
    pathlib.Path(opts.out).write_text(json.dumps(files, indent=1) + '\n')
    print(json.dumps({'pending': len(files)}))
    return 0


def merged(opts):
    """The final line of each file, and the files whose line is a rerun's."""
    final = {r['sha256']: r for r in load(opts.records)}
    rerun = {r['sha256']: r for r in load(opts.rerun or [])}
    final.update(rerun)
    return final, set(rerun)


def summarize(opts):
    final, rerun = merged(opts)
    expected = {e['sha256'] for e in json.loads(pathlib.Path(opts.files).read_text())}
    groups = {g: {'denominator': 0, 'counted': 0, 'misses': {}} for g in GROUPS}
    no_reference = collections.defaultdict(collections.Counter)
    other_codecs = collections.Counter()
    compared = ffmpeg_backend = unfinished = 0
    for sha, record in final.items():
        if sha not in expected:
            continue
        if record.get('host_limit'):
            unfinished += 1
            continue
        backend = (record.get('coreav') or {}).get('backend')
        ffmpeg_backend += backend == 'ffmpeg'
        if record['code'] in NO_REFERENCE:
            no_reference[record['group'] or 'none'][record['code']] += 1
            continue
        group = groups[record['group']]
        group['denominator'] += 1
        if record['group'] == 'other':
            other_codecs[record['codec']] += 1
        frames = record.get('frames')
        compared += min(frames['reference'], frames['coreav']) if frames else 0
        if record['counted']:
            group['counted'] += 1
            continue
        entry = group['misses'].setdefault(record['code'], {'files': 0, 'sub': {}})
        entry['files'] += 1
        if record['sub']:
            entry['sub'][record['sub']] = entry['sub'].get(record['sub'], 0) + 1
    missing = len(expected - set(final)) + unfinished
    outside = len(set(final) - expected)
    reasons_ = [{'reason': name, 'files': n} for name, n in (
        ('no_final_result', missing), ('ffmpeg_fallback_backend', ffmpeg_backend), ('file_outside_the_set', outside)) if n]
    denominator = sum(g['denominator'] for g in groups.values())
    counted = sum(g['counted'] for g in groups.values())
    for g in groups.values():
        g['share'] = g['counted'] / g['denominator'] if g['denominator'] else None
    summary = {'schema': 'coreav.codec-coverage-v1/1', 'bar_percent': BAR, 'files': len(expected),
               'denominator': denominator, 'counted': counted,
               'share': counted / denominator if denominator else None,
               'meets_bar': (counted * 100 >= BAR * denominator) if denominator and not reasons_ else None,
               'groups': groups, 'no_reference': {g: dict(c) for g, c in sorted(no_reference.items())},
               'other_codecs': dict(sorted(other_codecs.items())), 'frames_compared': compared,
               'files_rerun': len(rerun & expected), 'void': bool(reasons_), 'void_reasons': reasons_}
    pathlib.Path(opts.summary).write_text(json.dumps(summary, indent=1) + '\n')
    print(json.dumps({'denominator': denominator, 'counted': counted, 'void': bool(reasons_)}))
    return 2 if reasons_ else 0


def mismatches(opts):
    final, _ = merged(opts)
    lines = ['| sha256 | key | group | code | reference frames | CoreAV frames | first differing frame |',
             '|---|---|---|---|---|---|---|']
    for record in sorted(final.values(), key=lambda r: r['sha256']):
        if record.get('code') in ('frame_count', 'md5') and not record.get('host_limit'):
            f = record['frames']
            lines.append(f"| {record['sha256']} | {record['key']} | {record['group']} | {record['code']} | "
                         f"{f['reference']} | {f['coreav']} | {f['first_differing']} |")
    pathlib.Path(opts.out).write_text('\n'.join(lines) + '\n')
    print(json.dumps({'mismatches': len(lines) - 2}))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest='mode', required=True)
    p = sub.add_parser('run')
    for name in ('decode-oracle', 'ffmpeg', 'ffprobe', 'files', 'records'):
        p.add_argument('--' + name, required=True)
    p.add_argument('--shard')
    p.add_argument('--fetch')
    p.add_argument('--deadline', type=float)
    p.add_argument('--limit-minutes', type=float, default=150)
    p.add_argument('--rerun-host', action='store_true')
    p.add_argument('--jobs', type=int, default=1)
    p = sub.add_parser('pending')
    p.add_argument('--records', nargs='+', required=True)
    p.add_argument('--files', required=True)
    p.add_argument('--shard')
    p.add_argument('--out', required=True)
    for name, out in (('summarize', '--summary'), ('mismatches', '--out')):
        p = sub.add_parser(name)
        p.add_argument('--records', nargs='+', required=True)
        p.add_argument('--rerun', nargs='*')
        p.add_argument('--files', required=name == 'summarize')
        p.add_argument(out, required=True)
    opts = parser.parse_args()
    if opts.mode == 'run' and opts.fetch:
        import round_files
        pool = round_files.pool()
        try:
            return run(opts, lambda entry, directory: round_files.fetch_one(pool, entry, directory))
        finally:
            pool.close()
    if opts.mode == 'run':
        return run(opts)
    return {'pending': pending, 'summarize': summarize, 'mismatches': mismatches}[opts.mode](opts)


if __name__ == '__main__':
    sys.exit(main())
