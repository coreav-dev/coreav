#!/usr/bin/env python3
"""Compat check for `coreav probe --ffprobe-compat`.

For every file, runs `ffprobe -v error -of json -show_format -show_streams`
and `coreav probe --ffprobe-compat`, and compares every key CoreAV printed
with the same key in ffprobe's output, as the value ffprobe prints. Streams
are paired by `index`. Keys CoreAV leaves out are not compared.

Files that either tool rejects are not compared. A file in scope (MP4, MOV,
M4A) that ffprobe reads but CoreAV rejects fails the check unless the
overlay shows KD-STRICT checked on that file; the check also fails when no
file was compared. (Probe status through the binary is also gated by
scripts/oracle-coreav-cli.sh against the library adapter.)

A difference counts as agreement when the stream oracle's known-divergence
overlay (`scripts/oracle_kd.py --out`, or `scripts/oracle_kd_mkv.py --out`
for Matroska, passed as --kd-overlay) covered, on the same file and stream,
the field the key follows (FIELD_KEYS and FORMAT_KEYS below), and the
printed value is the value the entry was checked on, or its documented
conversion: the divergence is documented in docs/known-divergences.md (or
docs/known-divergences-mkv.md) and its predicate was checked on that field,
with the evidence its entry requires (for MP4/MOV entries, values read from
the file). Such keys are reported apart from direct agreement. For keys the
stream oracle does not gate, scripts/oracle_kd_mkv.py's COMPAT_ENTRIES
apply on Matroska files, with their evidence read from the file.

Usage:
  compat_check.py --coreav PATH --media DIR [--ffprobe PATH]
                  [--kd-overlay FILE] [--min-file-rate PERCENT] [--report FILE]
                  [--timeout SECONDS] [--in-scope .EXT,...]

--in-scope names the extensions whose rejection by CoreAV needs a known
divergence (default: MP4, MOV, M4A, the fixtures' scope); the stage 3 corpus
pass gives each file's own extension, so every rejection of a file ffprobe
reads is counted there. --timeout bounds each tool's run (default 120 s).

Both tools run with `GCONV_PATH` pointing at no gconv modules, as
scripts/oracle-ffprobe.sh does for the oracle's captures.

DIR holds the media files; a MANIFEST with `generated NAME` lines selects
them when present, otherwise every regular file is used. The check fails
(exit 1) when, for any key, the share of compared files on which that key
agrees is below --min-file-rate (default 100). Exit 2 is a harness error.
Uses only the Python standard library; ffprobe is a black box.
"""
import argparse
import json
import os
import pathlib
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import oracle_kd_mkv  # noqa: E402
import oracle_kd_ts  # noqa: E402

TIMEOUT = 120
IN_SCOPE = ('.mp4', '.mov', '.m4a')

# The compat keys that follow each field the stream oracle gates, for the
# field paths an overlay entry covered on a file (`covered_items[].paths`,
# scripts/oracle_kd.py and oracle_kd_mkv.py, #219): a compat key counts as
# covered only where the entry was checked on the field it follows, on the
# same stream. The format bit rate is the file size over the format
# duration, so it follows the duration.
FIELD_KEYS = {
    'duration_ts': {'streams.duration_ts', 'streams.duration'},
    'width': {'streams.width'},
    'height': {'streams.height'},
    'display_aspect_ratio': {'streams.display_aspect_ratio'},
    'kind': {'streams.codec_type'},
    'codec_name': {'streams.codec_name'},
    'profile': {'streams.profile'},
}
FORMAT_KEYS = {'format_duration_us': {'format.duration', 'format.bit_rate'}}
# A declared picture size the parameter sets contradict also moves the
# aspect ratios computed from it.
# The display ratio counts only with a sample aspect ratio ffprobe agrees
# with; the sample ratio itself is never excused by the size (#254 review).
SIZE_ENTRIES = {'KD-DECLARED-SIZE': {'streams.display_aspect_ratio'},
                'KD-MKV-DECLARED-SIZE': {'streams.display_aspect_ratio'}}


def covered_by_file(overlay_path):
    """file -> ({(compat key, stream index or None): (field, checked
    candidate value)}, set of KD ids) for divergences checked on it, field by
    field."""
    if overlay_path is None:
        return {}
    overlay = json.loads(overlay_path.read_text())
    items = overlay['levels']['streams']['all'].get('covered_items')
    if items is None:
        harness_error('overlay has no covered_items (old oracle_kd.py?)')
    covered = {}
    for item in items:
        if 'paths' not in item:
            harness_error('overlay has no covered field paths (old oracle_kd.py?)')
        keys, ids = covered.setdefault(item['file'], ({}, set()))
        ids.update(item['kd'])
        for path, checked in item['paths'].items():
            if not isinstance(checked, dict) or 'candidate' not in checked:
                harness_error('overlay has no checked values for its covered paths (old oracle_kd.py?)')
            parts = path.split('/')
            value = checked['candidate']
            if path in FORMAT_KEYS:
                keys.update(((key, None), (path, value)) for key in FORMAT_KEYS[path])
            elif len(parts) == 3 and parts[0] == 'streams' and parts[1].isdigit():
                index, field = int(parts[1]), parts[2]
                found = set(FIELD_KEYS.get(field, set()))
                if field in ('width', 'height'):
                    for kd in checked['kd']:
                        found |= SIZE_ENTRIES.get(kd, set())
                keys.update(((key, index), (field, value)) for key in found)
    return covered


def reduced(a, b):
    """`a:b` in lowest terms, as an aspect ratio."""
    from math import gcd
    g = gcd(a, b) or 1
    return f'{a // g}:{b // g}'


def bound(key, mine, field, value, ours, their_format, theirs=None):
    """Whether the compat value `mine` of `key` is what the oracle's checked
    candidate `value` of `field` gives: the same value, or the documented
    conversion of it. A covered field excuses a difference from ffprobe, not
    a compat output that differs from the value the entry was checked on
    (#219)."""
    try:
        if field == 'format_duration_us':
            if not isinstance(value, int):
                return False
            us = value
            if key == 'format.duration':
                return mine == f'{us // 1_000_000}.{us % 1_000_000:06d}'
            if key == 'format.bit_rate':
                # ffprobe's size: a wrong compat size fails as `size`.
                size = int(their_format.get('size'))
                return us > 0 and mine == str(size * 8_000_000 // us)
        if field == 'duration_ts':
            if key == 'streams.duration_ts':
                return mine == value and isinstance(mine, int)
            if key == 'streams.duration':
                # ffprobe's time base: a wrong compat one fails as
                # `time_base`.
                num, den = (int(x) for x in (theirs or {}).get('time_base', '').split('/'))
                return num == 1 and den > 0 and mine == f'{value * (1.0 / den):.6f}'
        if key == 'streams.display_aspect_ratio' and field in ('width', 'height'):
            # Derived from the size: the size must be the checked one, the
            # sample aspect ratio the one ffprobe reports, and the display
            # ratio that sample ratio applied to the size.
            if ours.get(field) != value or (theirs or {}).get('sample_aspect_ratio') != ours.get('sample_aspect_ratio'):
                return False
            n, d = (int(x) for x in ours.get('sample_aspect_ratio', '1:1').split(':'))
            return n > 0 and d > 0 and mine == reduced(int(ours['width']) * n, int(ours['height']) * d)
        return mine == value
    except (TypeError, ValueError, KeyError, AttributeError):
        return False


def explained(covered, name, key_path, mine=None, ours=None, their_format=None, theirs=None):
    """Whether the overlay explains `key_path` (`streams[3].duration`,
    `format.duration`) on file `name`, with the compat value `mine` bound to
    the value the entry was checked on (`ours`: the compat stream; `theirs`
    and `their_format`: ffprobe's stream and format)."""
    keys = covered.get(name, ({}, set()))[0]
    head, _, rest = key_path.partition('].')
    if head.startswith('streams[') and head[len('streams['):].isdigit():
        found = keys.get(('streams.' + rest, int(head[len('streams['):])))
    else:
        found = keys.get((key_path, None))
    if found is None:
        return False
    return bound(stream_key(key_path), mine, found[0], found[1], ours or {}, their_format or {}, theirs)


def harness_error(message):
    """Stops with exit 2, the harness's own failure, not a key's."""
    print(f'harness error: {message}', file=sys.stderr)
    sys.exit(2)


# The pinned static ffprobe converts DVB text (a transport stream's service
# names) with iconv, which loads the host's gconv modules; on a host whose
# glibc differs from the build's that crashes. With no modules to load,
# iconv fails cleanly and the names are left as read; no compared key
# depends on them. scripts/oracle-ffprobe.sh sets the same for the oracle's
# captures (docs/design/mpegts.md, "The oracle's environment").
CHILD_ENV = dict(os.environ, GCONV_PATH=os.environ.get('GCONV_PATH', '/nonexistent/gconv'))


def run_json(argv, timeout=TIMEOUT):
    """Runs argv; returns (exit code, parsed stdout or None)."""
    try:
        done = subprocess.run(argv, capture_output=True, timeout=timeout, check=False,
                              env=CHILD_ENV)
    except subprocess.TimeoutExpired:
        harness_error(f'timeout running {argv[0]}')
    if done.returncode != 0:
        return done.returncode, None
    try:
        return 0, json.loads(done.stdout)
    except json.JSONDecodeError:
        harness_error(f'{argv[0]} printed invalid JSON')


def media_files(directory):
    manifest = directory / 'MANIFEST'
    if manifest.exists():
        names = [line.split()[1] for line in manifest.read_text().splitlines()
                 if line.startswith('generated ')]
        return [directory / name for name in names]
    return sorted(p for p in directory.iterdir() if p.is_file())


def compare(path, ours, theirs, out):
    """Appends (key path, ours, theirs) for every key in `ours`."""
    if isinstance(ours, dict):
        for key, value in ours.items():
            other = theirs.get(key) if isinstance(theirs, dict) else None
            compare(f'{path}.{key}', value, other, out)
    elif isinstance(ours, list):
        for i, value in enumerate(ours):
            other = theirs[i] if isinstance(theirs, list) and i < len(theirs) else None
            compare(f'{path}[{i}]', value, other, out)
    else:
        out.append((path, ours, theirs))


# The overlay whose compat-only entries cover a file of each family.
COMPAT_OVERLAYS = {('.mkv', '.webm', '.mka', '.mk3d'): oracle_kd_mkv,
                   ('.ts', '.m2ts', '.mts', '.m2t'): oracle_kd_ts}


def compat_overlay(path):
    """The overlay for the file's family, else `None`."""
    return next((module for suffixes, module in COMPAT_OVERLAYS.items()
                 if path.name.endswith(suffixes)), None)


def compat_entry(path, key_path, theirs, ours):
    """Whether an entry for a key the stream oracle does not gate
    (an overlay's `COMPAT_ENTRIES`) covers `key_path` on file `path`, its
    evidence read from the file. `theirs` and `ours` are the two streams
    the comparison paired, which for a file whose streams the two readers
    list in a different order is not the pair at the same index."""
    overlay = compat_overlay(path)
    if overlay is None or ours is None:
        return False
    entry = overlay.COMPAT_ENTRIES.get(stream_key(key_path))
    head = key_path.partition('].')[0]
    if entry is None or not head.startswith('streams[') or not head[len('streams['):].isdigit():
        return False
    index = int(head[len('streams['):])
    return entry[1](str(path.parent), path.name, index, theirs, ours)


def stream_key(path):
    """`streams[3].tags.language` -> `streams.tags.language`."""
    head, _, rest = path.partition('].')
    if head.startswith('streams['):
        return 'streams.' + rest
    return path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--coreav', required=True)
    parser.add_argument('--ffprobe', default='ffprobe')
    parser.add_argument('--media', required=True, type=pathlib.Path)
    parser.add_argument('--kd-overlay', type=pathlib.Path)
    parser.add_argument('--min-file-rate', type=float, default=100.0)
    parser.add_argument('--report', type=pathlib.Path)
    parser.add_argument('--timeout', type=float, default=TIMEOUT)
    parser.add_argument('--in-scope', default=','.join(IN_SCOPE))
    args = parser.parse_args()
    in_scope = tuple(e for e in args.in_scope.split(',') if e)

    covered = covered_by_file(args.kd_overlay)
    per_key = {}  # key -> [agreeing files, compared files]
    via_kd = []
    diffs = []
    rejected = []
    unexplained = []
    compared_files = 0
    for path in media_files(args.media):
        ref_code, ref = run_json([args.ffprobe, '-v', 'error', '-of', 'json',
                                  '-show_format', '-show_streams', str(path)], args.timeout)
        our_code, ours = run_json([args.coreav, 'probe', '--ffprobe-compat', str(path)], args.timeout)
        if our_code not in (0, 1, 3):
            harness_error(f'coreav exited {our_code} on {path.name}')
        if ref is None or ours is None:
            entry = {'file': path.name, 'ffprobe_exit': ref_code, 'coreav_exit': our_code}
            excused = (ref is None or not path.name.endswith(in_scope)
                       or 'KD-STRICT' in covered.get(path.name, (set(), set()))[1])
            (rejected if excused else unexplained).append(entry)
            continue
        compared_files += 1
        pairs = []
        ref_streams = {s.get('index'): s for s in ref.get('streams', [])}
        # A file whose streams the two readers list in a different order
        # (KD-TS-STREAM-ORDER): pair them by the stream id both print,
        # which is the PID, so that each key is compared on the stream it
        # belongs to. `index` then differs, by that entry.
        ids = covered.get(path.name, ({}, set()))[1]
        by_id = {s.get('id'): s for s in ref.get('streams', [])}
        reordered = ('KD-TS-STREAM-ORDER' in ids
                     and len(by_id) == len(ref.get('streams', []))
                     and all(s.get('id') in by_id for s in ours.get('streams', [])))
        partner = ((lambda s: by_id.get(s.get('id'))) if reordered
                   else (lambda s: ref_streams.get(s.get('index'))))
        for stream in ours.get('streams', []):
            compare(f"streams[{stream.get('index')}]", stream, partner(stream), pairs)
        compare('format', ours.get('format', {}), ref.get('format', {}), pairs)
        file_keys = {}
        for key_path, mine, theirs in pairs:
            key = stream_key(key_path)
            ok = mine == theirs
            stream = None
            if key_path.startswith('streams['):
                stream = next((x for x in ours.get('streams', [])
                               if f"streams[{x.get('index')}]" == key_path.partition('.')[0]), None)
            ref_stream = partner(stream) if stream else None
            if (not ok and reordered and key == 'streams.index' and stream is not None
                    and mine == stream.get('index') and ref_stream is not None
                    and theirs == ref_stream.get('index')):
                # The entry's own difference: the stream the two readers
                # pair by its id sits at a different index on each side.
                via_kd.append({'file': path.name, 'key': key_path, 'coreav': mine,
                               'ffprobe': theirs, 'kd': 'KD-TS-STREAM-ORDER'})
                ok = True
            if not ok and explained(covered, path.name, key_path, mine, stream, ref.get('format', {}), ref_stream):
                via_kd.append({'file': path.name, 'key': key_path, 'coreav': mine, 'ffprobe': theirs})
                ok = True
            overlay = compat_overlay(path)
            if not ok and overlay is not None and compat_entry(path, key_path, ref_stream,
                                                               stream):
                via_kd.append({'file': path.name, 'key': key_path, 'coreav': mine, 'ffprobe': theirs,
                               'kd': overlay.COMPAT_ENTRIES[key][0]})
                ok = True
            file_keys[key] = file_keys.get(key, True) and ok
            if not ok:
                diffs.append({'file': path.name, 'key': key_path, 'coreav': mine, 'ffprobe': theirs})
        for key, ok in file_keys.items():
            agree, total = per_key.get(key, (0, 0))
            per_key[key] = (agree + int(ok), total + 1)

    failing = {}
    for key, (agree, total) in sorted(per_key.items()):
        rate = 100.0 * agree / total
        if rate < args.min_file_rate:
            failing[key] = {'agreeing_files': agree, 'files': total, 'rate': round(rate, 4)}
    report = {
        'compared_files': compared_files,
        'rejected_by_either': rejected,
        'rejected_without_known_divergence': unexplained,
        'keys': {k: {'agreeing_files': a, 'files': t} for k, (a, t) in sorted(per_key.items())},
        'failing_keys': failing,
        'diffs': diffs,
        'via_known_divergence': via_kd,
    }
    text = json.dumps(report, indent=2, sort_keys=True)
    if args.report:
        args.report.write_text(text + '\n')
    print(json.dumps({'compared_files': compared_files, 'keys': len(per_key),
                      'failing_keys': failing, 'diffs': len(diffs),
                      'via_known_divergence': len(via_kd),
                      'rejected_without_known_divergence': unexplained}, indent=2, sort_keys=True))
    return 1 if failing or unexplained or compared_files == 0 else 0


if __name__ == '__main__':
    sys.exit(main())
