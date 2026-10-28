#!/usr/bin/env python3
"""Checks `coreav check` (docs/design/policy-check.md) on a set of files.

For every file, `coreav probe --scan-fragments --scan-frame-sizes` runs
once (for Matroska and WebM files, with `--allow` naming every codec, as
`coreav check` probes them) and, with --ffprobe, `ffprobe -show_format
-show_streams` once (for Matroska and WebM files, also `-show_packets`). For every file and
every --policy:

- crash: `coreav check` exits 0 (accept) or 1 (reject) within TIMEOUT
  seconds and MEMORY bytes of address space, printing one verdict whose
  `verdict` agrees with its exit code and its reasons;
- determinism: a second run prints the same bytes with the same exit code;
- spec: the reasons equal those of scripts/policy_ref.py, a second
  implementation written from the schema documents, over the file's probe
  output: every key but `message` and `error`, in order (the overall bit
  rate's convenience `value` within a relative 1e-12, since it is a
  rounded copy of `exact`). A file the probe rejects has exactly one
  `unreadable` reason whose `error` is the object `coreav probe` printed
  (for a transport stream, which `coreav check` does not read whatever the
  probe makes of it, an `unsupported_container` error);
- precision: every reason's `path` exists in the probe output, and its
  `value` is the value there (or the documented derivation of it); an
  `unknown` reason points at a `null`;
- oracle (with --ffprobe, for files both read): the failed rules, as
  `policy_ref.outcome` lists them, equal those the same evaluator finds in
  ffprobe's output (`policy_ref.from_ffprobe`, with --oracle-frames also
  the largest decoded frame sizes, and with fragmentation from an
  independent box walk; `policy_ref.from_ffprobe_matroska` with the
  `DocType`, live flag and track entries from an EBML walk and each
  stream's span and bytes from its packets), over the rules the oracle can
  observe
  (`policy_ref.unobservable`). A disagreement that an entry of
  docs/known-divergences-policy.md covers (`policy_ref.covered`) is
  reported apart from direct agreement.

- hostile (with --media holding a hostile-suite manifest.json,
  tools/hostile/generate.py): a file marked `reject` gets a
  reject verdict under every policy.

The report lists every failure. Exit 0 when crash, determinism, spec and
precision hold for every case and oracle agreement is at least
--oracle-bar; 1 otherwise; 2 for a harness error.

Usage: policy_check.py --coreav BIN --policy P [--policy P ...]
       (--media DIR | --list FILE) [--ffprobe BIN] [--report OUT]
The binaries are black boxes; no implementation source is read. Uses only
the Python standard library.
"""
import argparse
import json
import math
import pathlib
import resource
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import policy_ref  # noqa: E402

TIMEOUT = 10
MEMORY = 1 << 30
REASON_KEYS = ('code', 'rule', 'path', 'stream_index', 'value', 'exact', 'limit')


def limit_memory():
    resource.setrlimit(resource.RLIMIT_AS, (MEMORY, MEMORY))


def run(argv, timeout=TIMEOUT):
    try:
        done = subprocess.run(argv, capture_output=True, timeout=timeout, check=False,
                              preexec_fn=limit_memory)
    except subprocess.TimeoutExpired:
        return None
    return done


def resolve(doc, path):
    """(found, value) for an RFC 6901 pointer. A pointer that runs into a
    `null` (a stream's `video` object, for example) resolves to `null`, as
    verdict-v1.md lets an `unknown` reason point there."""
    node = doc
    for part in path.split('/')[1:]:
        part = part.replace('~1', '/').replace('~0', '~')
        if node is None:
            return True, None
        if isinstance(node, list) and part.isdigit() and int(part) < len(node):
            node = node[int(part)]
        elif isinstance(node, dict) and part in node:
            node = node[part]
        else:
            return False, None
    return True, node


def is_matroska(path):
    try:
        with open(path, 'rb') as f:
            return f.read(4) == b'\x1a\x45\xdf\xa3'
    except OSError:
        return False


def packets(ffprobe, path, streams):
    """Per stream index, (span, bytes) of its packets, from `ffprobe
    -show_packets` (policy_ref.packet_spans); None when ffprobe fails."""
    done = run([ffprobe, '-v', 'error', '-of', 'csv=p=0', '-show_entries',
                'packet=stream_index,pts,dts,duration,size', str(path)], timeout=600)
    if done is None or done.returncode != 0:
        return None
    bases = {}
    for s in streams:
        num, _, den = str(s.get('time_base', '')).partition('/')
        if num.isdigit() and den.isdigit() and int(den):
            bases[s.get('index')] = policy_ref.Fraction(int(num), int(den))

    def number(text):
        try:
            return int(text)
        except ValueError:
            return None

    rows = []
    for line in done.stdout.decode('utf-8', 'replace').splitlines():
        fields = line.split(',')
        if len(fields) < 5:
            continue
        # ffmpeg gives the packets of some tracks (V_MS/VFW/FOURCC) decode
        # times only: those count at their dts.
        pts = number(fields[1])
        rows.append((number(fields[0]), pts if pts is not None else number(fields[2]),
                     number(fields[3]), number(fields[4])))
    return policy_ref.packet_spans(rows, bases)


def precision(reason, doc):
    """None when the reason's value is what its path holds."""
    path, rule, code = reason['path'], reason['rule'], reason['code']
    if path is None:
        return None
    found, at = resolve(doc, path)
    if not found:
        return f'{path} is not in the probe output'
    if rule == '/duration/max_seconds' and isinstance(at, dict):
        at = at.get('seconds') if at.get('ticks') is not None else None
    elif rule and rule.endswith('/max_bit_rate') and rule != '/file/max_bit_rate' \
            and isinstance(at, dict):
        at = at.get('bits_per_second') if at.get('exact') is not None else None
    elif rule == '/container/formats' and code != 'unknown' and path != '/container/doc_type':
        at = policy_ref.container_format(at)
    elif path.endswith('/codec') and rule and rule.count('/') == 2 and code != 'unknown':
        # A kind that is not allowed: the family of the codec at `path`.
        at = policy_ref.FAMILIES.get(at)
    if code == 'unknown':
        return None if at is None else f'unknown, but {path} holds {at!r}'
    return None if at == reason['value'] else f'value {reason["value"]!r}, {path} holds {at!r}'


def same_reasons(got, want):
    if len(got) != len(want):
        return False
    for g, w in zip(got, want):
        for key in REASON_KEYS:
            a, b = g.get(key), w.get(key)
            if key == 'value' and g.get('rule') == '/file/max_bit_rate' \
                    and isinstance(a, float) and isinstance(b, float):
                if not math.isclose(a, b, rel_tol=1e-12):
                    return False
            elif a != b:
                return False
    return True


def channel_mismatches(doc, mapped):
    """Audio streams whose channel count differs between the probe output
    and ffprobe: (stream_index, coreav, ffprobe). Informational."""
    theirs = {s['stream_index']: (s.get('audio') or {}).get('channels')
              for s in mapped['streams'] if s['kind'] == 'audio'}
    found = []
    for s in doc.get('streams', []):
        if s.get('kind') == 'audio' and s.get('stream_index') in theirs:
            ours = (s.get('audio') or {}).get('channels')
            if ours != theirs[s['stream_index']]:
                found.append([s['stream_index'], ours, theirs[s['stream_index']]])
    return found


def frame_sizes(ffprobe, path):
    """Stream index to the largest decoded frame size, from `ffprobe
    -show_frames` (decodes every frame: for small files only)."""
    done = run([ffprobe, '-v', 'error', '-of', 'json', '-show_entries',
                'frame=stream_index,width,height', str(path)], timeout=120)
    if done is None or done.returncode != 0:
        return None
    sizes = {}
    for frame in json.loads(done.stdout).get('frames', []):
        if isinstance(frame.get('width'), int) and isinstance(frame.get('height'), int):
            w, h = sizes.get(frame.get('stream_index'), (0, 0))
            sizes[frame.get('stream_index')] = (max(w, frame['width']), max(h, frame['height']))
    return sizes


# The probe documents `coreav check` evaluates a policy against.
CHECKED_SCHEMAS = (policy_ref.PROBE_SCHEMA, policy_ref.MATROSKA_SCHEMA)
# The packet sizes of a transport stream and the bytes before each packet
# (ISO/IEC 13818-1 2.4.3.2; the BDAV arrival timestamp of 192-byte
# packets), as `coreav_mpegts::sniff` reads them.
TS_LAYOUTS = ((188, 0), (192, 4), (204, 0))


def is_mpegts(path):
    """Whether the file starts with stored transport packets: the sync byte
    at the start of three consecutive packets of one of the sizes, or of
    the packets a file too short for three can hold, as the reader's own
    detection counts them (`coreav_mpegts::detect`)."""
    with open(path, 'rb') as handle:
        head = handle.read(4 + 3 * 204)
    for size, prefix in TS_LAYOUTS:
        seen = 0
        for k in range(3):
            at = prefix + k * size
            if at + 188 > len(head):
                break
            if head[at] != 0x47:
                seen = 0
                break
            seen += 1
        if seen:
            return True
    return False


def check_file(path, policies, coreav, ffprobe, oracle_frames=False):
    records = []
    mkv = is_matroska(path)
    argv = [coreav, 'probe', '--scan-fragments', '--scan-frame-sizes']
    if mkv:
        argv += ['--allow', ','.join(policy_ref.MATROSKA_CODECS)]
    probed = run(argv + [str(path)])
    doc = error = None
    if probed is not None and probed.returncode == 0:
        doc = json.loads(probed.stdout)
    elif probed is not None and probed.returncode == 1:
        error = json.loads(probed.stderr)
    # `coreav check` reads ISO base media and Matroska files only
    # (`coreav-policy`). A transport stream is a container it does not
    # read, whatever `coreav probe` makes of it, so the reference is the
    # one `unreadable` reason with an `unsupported_container` error.
    if doc is not None and doc.get('schema') not in CHECKED_SCHEMAS or is_mpegts(path):
        doc, error = None, {'kind': 'unsupported_container'}
    ff = spans = None
    walked = None if mkv else policy_ref.fragmented(path)
    headers = policy_ref.matroska_headers(path) if mkv else policy_ref.movie_headers(path)
    if ffprobe:
        done = run([ffprobe, '-v', 'error', '-of', 'json', '-show_format', '-show_streams',
                    str(path)], timeout=60)
        if done is not None and done.returncode == 0:
            try:
                ff = json.loads(done.stdout)
            except json.JSONDecodeError:
                ff = None
        if ff is not None and mkv:
            spans = packets(ffprobe, path, ff.get('streams', []))
            if spans is None or headers is None:
                ff = None
    for name, text, policy in policies:
        record = {'file': path.name, 'policy': name, 'failures': []}
        fail = record['failures'].append
        first = run([coreav, 'check', '--policy', text, str(path)])
        second = run([coreav, 'check', '--policy', text, str(path)])
        if first is None or second is None:
            fail('crash: timeout')
            records.append(record)
            continue
        if first.returncode not in (0, 1):
            fail(f'crash: exit code {first.returncode}: {first.stderr[-300:]!r}')
            records.append(record)
            continue
        if (first.returncode, first.stdout) != (second.returncode, second.stdout):
            fail('determinism: two runs differ')
        try:
            verdict = json.loads(first.stdout)
        except json.JSONDecodeError:
            fail('crash: no verdict on stdout')
            records.append(record)
            continue
        reasons = verdict.get('reasons', [])
        accepted = verdict.get('verdict') == 'accept'
        record['verdict'] = verdict.get('verdict')
        record['rules'] = [r.get('rule') for r in reasons]
        if verdict.get('schema') != 'coreav.verdict/1' or accepted != (first.returncode == 0) \
                or accepted != (not reasons):
            fail('crash: verdict, exit code and reasons disagree')
        if doc is None:
            record['unreadable'] = error.get('kind') if error else 'probe-crash'
            got = reasons[0].get('error') if len(reasons) == 1 else None
            expected = got == error if len(error) > 1 else (
                isinstance(got, dict) and got.get('kind') == error['kind'])
            if not (len(reasons) == 1 and reasons[0].get('code') == 'unreadable' and expected):
                fail('spec: an unreadable file needs one unreadable reason with the probe error')
            records.append(record)
            continue
        want = policy_ref.evaluate(policy, doc)
        if not same_reasons(reasons, want):
            fail({'spec': {'coreav': [{k: r.get(k) for k in REASON_KEYS} for r in reasons],
                           'reference': want}})
        for reason in reasons:
            problem = precision(reason, doc)
            if problem:
                fail(f'precision: {problem}')
        if ff is not None:
            frames = frame_sizes(ffprobe, path) if oracle_frames else None
            if mkv:
                excluded = set(policy_ref.UNOBSERVABLE)
                mapped = policy_ref.from_ffprobe_matroska(ff, headers, spans, frames)
            else:
                excluded = policy_ref.unobservable(policy, walked)
                mapped = policy_ref.from_ffprobe(ff, walked, headers, frames)
            record['channels'] = channel_mismatches(doc, mapped)
            ours = policy_ref.outcome(reasons, excluded)
            theirs = policy_ref.outcome(
                policy_ref.evaluate(policy_ref.observable(policy, excluded), mapped), excluded)
            record['oracle'] = ours == theirs
            record['oracle_excluded'] = sorted(excluded - policy_ref.UNOBSERVABLE)
            if ours != theirs:
                record['oracle_diff'] = {'coreav': ours, 'ffprobe': theirs}
                if mkv:
                    evidence = {'matroska': headers, 'spans': spans,
                                'laced': policy_ref.laced_tracks(path),
                                'audit': policy_ref.matroska_audit(path)}
                else:
                    entries = policy_ref.track_entries(path)
                    evidence = {'protected': policy_ref.protected_streams(path, entries),
                                'timing': policy_ref.track_timing(path),
                                'entries': entries, 'headers': headers,
                                'fragmented': walked,
                                'mehd': policy_ref.fragment_duration(path) if walked else None,
                                'origins': policy_ref.fragment_origins(path) if walked else {}}
                kds = policy_ref.covered(policy, excluded, mapped, evidence, ours, mkv)
                if kds:
                    record['oracle_covered_by'] = kds
        records.append(record)
    return records


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--coreav', required=True)
    parser.add_argument('--policy', action='append', required=True, type=pathlib.Path)
    parser.add_argument('--media', type=pathlib.Path)
    parser.add_argument('--list', type=pathlib.Path, help='file with one path per line')
    parser.add_argument('--ffprobe')
    parser.add_argument('--oracle-bar', type=float, default=1.0)
    parser.add_argument('--oracle-frames', action='store_true',
                        help='decode every frame with ffprobe for the oracle sizes (small files)')
    parser.add_argument('--report', type=pathlib.Path)
    args = parser.parse_args()
    if args.media:
        files = sorted(p for p in args.media.iterdir()
                       if p.is_file() and p.name not in ('MANIFEST', 'manifest.json'))
    elif args.list:
        files = [pathlib.Path(line) for line in args.list.read_text().split('\n') if line]
    else:
        print('harness error: give --media or --list', file=sys.stderr)
        return 2
    policies = []
    for path in args.policy:
        text = path.read_text()
        policies.append((path.stem, str(path), policy_ref.load_policy(text)))
    if not files:
        print('harness error: no files', file=sys.stderr)
        return 2
    must_reject = set()
    manifest = args.media / 'manifest.json' if args.media else None
    if manifest and manifest.is_file():
        cases = json.loads(manifest.read_text()).get('cases', [])
        must_reject = {c['name'] for c in cases if c.get('outcome') == 'reject'}
    records = []
    for path in files:
        for record in check_file(path, policies, args.coreav, args.ffprobe, args.oracle_frames):
            if path.name in must_reject and record.get('verdict') == 'accept':
                record['failures'].append('hostile: a file the suite marks reject was accepted')
            records.append(record)
    summary = summarize(records)
    report = {'summary': summary, 'records': records}
    if args.report:
        args.report.write_text(json.dumps(report, indent=1))
    print(json.dumps(summary, indent=2))
    failed = summary['failures']
    oracle = summary['oracle']
    agreeing = oracle['agree'] + oracle['covered']
    low = oracle['compared'] and agreeing < args.oracle_bar * oracle['compared']
    return 1 if failed or low else 0


def summarize(records):
    by_kind = {}
    for r in records:
        for f in r['failures']:
            kind = f.split(':')[0] if isinstance(f, str) else next(iter(f))
            by_kind[kind] = by_kind.get(kind, 0) + 1
    compared = [r for r in records if 'oracle' in r]
    return {
        'cases': len(records),
        'files': len({r['file'] for r in records}),
        'accept': sum(r.get('verdict') == 'accept' for r in records),
        'reject': sum(r.get('verdict') == 'reject' for r in records),
        'unreadable': sum('unreadable' in r for r in records),
        'failures': sum(len(r['failures']) for r in records),
        'failures_by_check': by_kind,
        'oracle': {'compared': len(compared), 'agree': sum(r['oracle'] for r in compared),
                   'covered': sum(bool(r.get('oracle_covered_by')) for r in compared),
                   'disagreements': [{'file': r['file'], 'policy': r['policy'],
                                      'covered_by': r.get('oracle_covered_by'),
                                      **r['oracle_diff']}
                                     for r in compared if not r['oracle']]},
    }


if __name__ == '__main__':
    sys.exit(main())
