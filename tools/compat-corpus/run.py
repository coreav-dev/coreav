#!/usr/bin/env python3
"""Runs the command-coverage set against `coreav ffprobe` and the pinned
`ffprobe` (docs/design/compat-cli.md, "The measurements").

    run.py commands --coreav PATH --media DIR [DIR...] [--report FILE]
        Every `ffprobe` command of commands-v1 on every file of the
        directories (a MANIFEST with `generated NAME` lines selects the
        files when present). `out-of-scope` commands: coreav must exit
        non-zero, print a message on standard error and nothing on standard
        output (B1). `native` commands: coreav must exit 0 (B2) and print
        what ffprobe prints, byte for byte, on every file ffprobe reads (B3a
        on the fixtures). Exit 1 when any bar is missed.

    run.py keys --coreav PATH --media DIR [DIR...] [--list FILE] [--report FILE]
        Per file, both tools print `-show_format -show_streams
        -show_chapters -show_programs` and `-count_packets -show_packets`
        in JSON; every key ffprobe prints is compared (B3a on the corpus).
        Reports, per key, the share of files it appears in on which every
        occurrence agrees.

    run.py commands --set 0.1.0 --coreav PATH --media DIR [DIR...] [--report FILE]
        R1 (docs/design/compat-cli.md, "The 0.1.0 bars"): every `ffprobe`
        command of commands-0.1.0 on every file of its container families.
        Standard output must be identical to ffprobe's on every file ffprobe
        reads, after the known divergences B3a sets aside (the fixtures of
        SET_ASIDE, and a rejection KD-STRICT covers); there is no baseline, so
        every difference misses, and so does a (command, family) pair of a
        family given whose files ffprobe reads none of. Counts per command and
        per family; exit 1 when any pair misses, 2 when the run is void (its
        fallback log holds a line), 3 when it measured nothing (no file of
        the set's families, or none that ffprobe reads).

    run.py keys --set 0.1.0 --coreav PATH --media DIR [DIR...] [--list FILE]
                [--records FILE] [--report FILE] [--summary FILE]
        R2: the comparison of `keys`, counting only the keys the set prints,
        per container family, as B3a's rounds count them: a key found in at
        least five files of a family is gated alone, and the tag keys found
        in fewer are pooled into one row gated on their occurrences.
        --records writes one line per file (its name, family and keys:
        private, as the files are); --summary writes the counts alone, with
        no file named. Exit 1 when a gated row of a family is below the bar
        or a family given has no file that ffprobe reads, 2 when the run is
        void, 3 when it measured nothing (no file that ffprobe reads).

    run.py keys-report [--records FILE...] [--summaries FILE...] [--report FILE] [--summary FILE]
        The same report from the records or the summaries of several
        `keys --set 0.1.0` runs, for a run split over hosts: a summary holds
        the counts R2 is judged on and names no file, so summaries add up.
        Exit statuses as `keys`: no input, or inputs with no file that
        ffprobe reads, is 3.

Both tools run with no gconv modules (release_round.NO_GCONV), and in the
0.1.0 modes with a fallback log of the run. ffprobe is a black box: only
its output is read. Python 3 standard library only.
"""
import argparse
import collections
import concurrent.futures
import fractions
import functools
import importlib.util
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import release_round  # noqa: E402

COMMANDS = HERE / 'commands-v1' / 'commands.jsonl'
TIMEOUT = 600
# `coreav` in strict-safe mode: a command it does not run natively is
# refused, never handed to the `ffprobe` on PATH, which would agree with
# itself (docs/design/compat-cli.md, "Fallback to FFmpeg").
ENV = release_round.environment()


def media_files(dirs):
    out = []
    for d in dirs:
        d = pathlib.Path(d)
        if d.is_file():
            out.append(d)
            continue
        manifest = d / 'MANIFEST'
        if manifest.exists():
            for line in manifest.read_text().splitlines():
                parts = line.split()
                if len(parts) >= 2 and parts[0] == 'generated':
                    out.append(d / parts[1])
        else:
            out.extend(sorted(p for p in d.iterdir()
                              if p.is_file() and p.suffix not in ('.json', '.log', '')))
    return out


def run(cmd):
    try:
        p = subprocess.run(cmd, capture_output=True, timeout=TIMEOUT, env=ENV)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return None, b'', b'timeout'


def substitute(args, path):
    return [str(path) if a == 'INPUT' else a for a in args]


def load_commands():
    with open(COMMANDS) as f:
        return [json.loads(line) for line in f if line.strip()]


# Fixtures crafted to show a documented divergence: the record, and the keys
# it names. A difference on such a fixture is set aside, and counted apart,
# only when every key the two tools print differently over the whole file
# (the stream listing and the packet listing of `run.py keys`) is one the
# record names; `packets.*` stands for whole packets, which the record lets
# one side list and the other not. Any other difference fails.
DECLARED_SIZE = ('KD-DECLARED-SIZE (docs/known-divergences.md)',
                 {'streams.width', 'streams.height', 'streams.display_aspect_ratio'})
# ffprobe drops the parameter set, so it prints none of what the set gives.
HEVC_WINDOW = ('KD-HEVC-WINDOW (docs/known-divergences.md)',
               {'streams.profile', 'streams.level', 'streams.color_range', 'streams.pix_fmt',
                'streams.chroma_location', 'streams.coded_width', 'streams.coded_height',
                'streams.has_b_frames'})
SET_ASIDE = {
    'size_entry_understated.mp4': DECLARED_SIZE,
    'size_prores_understated.mov': DECLARED_SIZE,
    'size_hevc_window.mp4': HEVC_WINDOW,
    'size_hevc_mixed_window.mp4': HEVC_WINDOW,
    'ts_duplicates.ts': ('KD-TS-DUPLICATE (docs/known-divergences-ts.md)',
                         {'packets.*', 'streams.bit_rate', 'programs.streams.bit_rate'}),
}


def covered_by_record(coreav, path):
    """The record that covers every difference of `path`, or None. Keys
    CoreAV prints and ffprobe does not, and a packet listing of another
    length, count as differences too."""
    record, fields = SET_ASIDE.get(path.name, (None, set()))
    if record is None:
        return None
    fields = fields | set(ffmpeg9().get(path.name, {}))
    _, ref_out, _ = run(['ffprobe'] + SECTION_ARGS + [str(path)])
    code, out, _ = run([coreav, 'ffprobe'] + SECTION_ARGS + [str(path)])
    try:
        extra = {key_name(k) for k in set(flatten(json.loads(out))) - set(flatten(json.loads(ref_out)))}
    except ValueError:
        return None
    extra -= fields
    if code != 0 or extra:
        return None
    if 'packets.*' not in fields:
        listings = [run(tool + PACKET_ARGS + [str(path)])[1].count(b'\n')
                    for tool in (['ffprobe'], [coreav, 'ffprobe'])]
        if listings[0] != listings[1]:
            return None
    for args in (SECTION_ARGS, PACKET_ARGS):
        r = compare_file(coreav, path, args)
        if r is None or r[2] != 0:
            return None
        for name, ok in r[0].items():
            if ok or name in fields or ('packets.*' in fields and name.startswith('packets.')):
                continue
            return None
    return record


def cmd_commands(opts):
    files = media_files(opts.media)
    for path in files:
        av1_nclx_fixture(opts.coreav, path)
        empty_edit_padding_fixture(opts.coreav, path)
    records = [r for r in load_commands() if r['tool'] == 'ffprobe']
    aside = {p.name: covered_by_record(opts.coreav, p) for p in files if p.name in SET_ASIDE}
    set_aside = collections.Counter()

    b1 = collections.Counter()
    b2 = collections.Counter()
    b3 = collections.Counter()
    failures = []
    for r in records:
        if r['label'] == 'unusable':
            continue
        args = r['args']
        if r['label'] == 'out-of-scope':
            # A refusal does not depend on the file: one is enough.
            target = files[0] if files else 'INPUT'
            code, out, err = run([opts.coreav, 'ffprobe'] + substitute(args, target))
            ok = code not in (0, None) and not out and err.strip()
            b1['pass' if ok else 'fail'] += 1
            if not ok:
                failures.append({'bar': 'B1', 'args': args, 'exit': code,
                                 'stdout': out[:200].decode('utf-8', 'replace')})
            continue
        def both(path):
            argv = substitute(args, path)
            ref_code, ref_out, _ = run(['ffprobe'] + argv)
            if ref_code != 0:
                return path, None, (None, None)
            got = run([opts.coreav, 'ffprobe'] + argv)
            if got[0] != 0 or got[1] == ref_out or aside.get(path.name):
                return path, (ref_out,) + got, (None, None)
            return path, (ref_out,) + got, ffmpeg9_covers(opts.coreav, args, path, ref_out, got[1])
        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            pairs = list(pool.map(both, files))
        for path, pair, (entries, keys) in pairs:
            if pair is None:
                continue
            ref_out, code, out, err = pair
            if code != 0:
                b2['fail'] += 1
                failures.append({'bar': 'B2', 'args': args, 'file': path.name, 'exit': code,
                                 'stderr': err[:300].decode('utf-8', 'replace')})
                continue
            b2['pass'] += 1
            if out == ref_out:
                b3['pass'] += 1
            elif aside.get(path.name):
                b3['set aside'] += 1
                set_aside[aside[path.name]] += 1
            elif entries:
                b3['set aside'] += 1
                set_aside.update(entries)
            else:
                b3['fail'] += 1
                failures.append({'bar': 'B3', 'args': args, 'file': path.name,
                                 'diff': first_difference(ref_out, out), 'keys': keys})
    b1_ffmpeg, _ = ffmpeg_refusals(opts, files, collections.Counter(), failures)
    report = {'B1': dict(b1), 'B1 ffmpeg': dict(b1_ffmpeg), 'B2': dict(b2), 'B3': dict(b3),
              'set aside': dict(set_aside), 'failures': failures}
    if opts.report:
        pathlib.Path(opts.report).write_text(json.dumps(report, indent=1))
    print(json.dumps({k: report[k] for k in ('B1', 'B1 ffmpeg', 'B2', 'B3', 'set aside')}))
    by = collections.Counter((f['bar'], ' '.join(f['args'])) for f in failures)
    for (bar, args), n in by.most_common(opts.top):
        print(f'{bar} x{n}: {args}')
    found = sorted({failure_key(f) for f in failures})
    if opts.write_baseline:
        pathlib.Path(opts.write_baseline).write_text(json.dumps(
            {'note': 'failing (bar, command, fixture) triples of tools/compat-corpus/run.py '
                     'commands on the generated fixtures; CI fails on any other',
             'failing': found}, indent=1) + '\n')
    if opts.baseline:
        known = set(json.loads(pathlib.Path(opts.baseline).read_text())['failing'])
        new = [k for k in found if k not in known]
        fixed = [k for k in known if k not in set(found)]
        by_key = {failure_key(f): f for f in failures}
        for k in new[:opts.top]:
            print(f'NEW FAILURE: {k}')
            detail = by_key.get(k, {})
            if 'diff' in detail or 'stderr' in detail or 'exit' in detail:
                print('    ' + json.dumps({x: detail.get(x) for x in ('diff', 'exit', 'stderr') if x in detail})[:600])
        print(f'{len(new)} new failures, {len(fixed)} fixed since the baseline')
        return 1 if new else 0
    return 1 if failures else 0


def failure_key(f):
    """One failure, as the baseline lists it: the bar, the command, and the
    fixture (none for a refusal, which does not depend on the file)."""
    return '|'.join([f['bar'], ' '.join(f['args']), f.get('file', '')])


HELP_QUERIES = [[flag] for flag in ('-h', '-?', '-help', '--help')]
# `coreav ffmpeg`'s usage text, as docs-site/snippets/help/ffmpeg.mdx shows
# it; crates/coreav-cli/tests/docs_site.rs keeps the snippet equal to it.
FFMPEG_USAGE = HERE.parents[1] / 'docs-site' / 'snippets' / 'help' / 'ffmpeg.mdx'


@functools.cache
def ffmpeg_usage():
    text = FFMPEG_USAGE.read_text()
    return text.removeprefix('```text\n').removesuffix('```\n').encode()


def ffmpeg_refusals(opts, files, b1, failures):
    """B1 for `coreav ffmpeg`: every out-of-scope `ffmpeg` command of the
    set exits non-zero with a message, prints nothing and writes no output
    file. The output placeholders name files in a new directory, which
    must stay empty. A help query with no topic (`-h`, `--help`) is
    counted apart, as `help`: it must exit 0, print `coreav ffmpeg`'s
    whole usage text and nothing on standard error, and write no file
    (docs/design/compat-cli.md, "Help")."""
    import tempfile
    target = str(files[0]) if files else 'INPUT'
    for r in load_commands():
        if r['tool'] != 'ffmpeg' or r['label'] != 'out-of-scope':
            continue
        with tempfile.TemporaryDirectory() as tmp:
            argv = []
            for a in r['args']:
                if a.startswith('INPUT'):
                    argv.append(target)
                elif a in ('OUTPUT', 'FILE'):
                    argv.append(str(pathlib.Path(tmp) / f'{a.lower()}.mp4'))
                else:
                    argv.append(a)
            code, out, err = run([opts.coreav, 'ffmpeg'] + argv)
            wrote = any(pathlib.Path(tmp).iterdir())
        if argv in HELP_QUERIES:
            ok = code == 0 and out == ffmpeg_usage() and not err and not wrote
            b1['help' if ok else 'fail'] += 1
        else:
            ok = code not in (0, None) and not out and err.strip() and not wrote
            b1['pass' if ok else 'fail'] += 1
        if not ok:
            failures.append({'bar': 'B1', 'tool': 'ffmpeg', 'args': r['args'], 'exit': code})
    return b1, failures


def first_difference(a, b):
    la, lb = a.decode('utf-8', 'replace').splitlines(), b.decode('utf-8', 'replace').splitlines()
    for i in range(max(len(la), len(lb))):
        x = la[i] if i < len(la) else '<end>'
        y = lb[i] if i < len(lb) else '<end>'
        if x != y:
            return {'line': i + 1, 'ffprobe': x[:300], 'coreav': y[:300]}
    return {'line': None}


# Differences that come from FFmpeg 9.0.2 itself, not from CoreAV: keys it
# prints that `coreav ffprobe` does not yet print, keys it no longer prints,
# and values it gives otherwise. ffmpeg9.json names each key and the files it
# differs on, under the entry of docs/known-divergences-ffprobe9.md that says
# why. A B3 difference is set aside only when the two outputs are byte for
# byte the same once those keys of that file, and nothing else, are taken out
# of both, in the command's own output format, and every value that differs
# on those keys is one the entry accepts (its `accepts`, [accepts]).
FFMPEG9 = HERE / 'ffmpeg9.json'
WRITER_FLAGS = ('-of', '-print_format', '-output_format')
WRITER_OPTIONS = {'nokey': 'nk', 'noprint_wrappers': 'nw', 'print_section': 'p',
                  'item_sep': 's', 'sep_char': 's', 'escape': 'e', 'compact': 'c'}
WRITER_DEFAULTS = {'default': {'nk': '0', 'nw': '0'},
                   'compact': {'nk': '0', 'p': '1', 's': '|', 'e': 'c'},
                   'csv': {'nk': '1', 'p': '1', 's': ',', 'e': 'csv'},
                   'flat': {'s': '.'},
                   'json': {}}
# The options under which a writer shows every key, section name and wrapper.
WRITER_FULL = {'default': {'nk': '0', 'nw': '0'},
               'compact': {'nk': '0', 'p': '1'},
               'csv': {'nk': '0', 'p': '1'},
               'flat': {'s': '.'},
               'json': {}}


@functools.cache
def ffmpeg9():
    """{file name: {key: entry id}} from ffmpeg9.json."""
    out = collections.defaultdict(dict)
    for entry in json.loads(FFMPEG9.read_text())['entries']:
        for key, names in entry['keys'].items():
            for name in names:
                out[name][key] = entry['id']
    return out


@functools.cache
def ffmpeg9_rules():
    """{entry id: {key: rule}} from ffmpeg9.json."""
    return {entry['id']: entry['accepts'] for entry in json.loads(FFMPEG9.read_text())['entries']}


@functools.cache
def av1_nclx_fixture(coreav, path):
    """Validate the one AV1 fixture's bytes and exact oracle disagreement;
    an oracle that starts agreeing must retire this exception."""
    if path.name != 'color_av1_colr_ignored.mp4':
        return False
    prefix = 'KD-FF9-AV1-NCLX: '
    try:
        if path.stat().st_size > 8 * 1024 * 1024:
            raise ValueError('fixture exceeds the evidence reader limit')
        data = path.read_bytes()

        def one(start, end, kind):
            found = []
            while start < end:
                if start + 8 > end:
                    raise ValueError('truncated box header')
                size = int.from_bytes(data[start:start + 4], 'big')
                if size < 8 or start + size > end:
                    raise ValueError('changed or invalid box layout')
                if data[start + 4:start + 8] == kind:
                    found.append((start + 8, start + size))
                start += size
            if len(found) != 1:
                raise ValueError('expected exactly one ' + kind.decode('ascii') + ' box')
            return found[0]

        start, end = 0, len(data)
        for kind in (b'moov', b'trak', b'mdia', b'minf', b'stbl', b'stsd'):
            start, end = one(start, end, kind)
        if data[start:start + 8] != b'\0\0\0\0\0\0\0\1':
            raise ValueError('changed sample-description header')
        start, end = one(start + 8, end, b'av01')
        start += 78
        a, b = one(start, end, b'colr')
        colour = data[a:b]
        if colour != b'nclx\0\6\0\6\0\6\x80':
            raise ValueError('changed nclx colour or range bytes')
        a, b = one(start, end, b'av1C')
        record = data[a:b]
        if len(record) < 4 or record[0] != 0x81:
            raise ValueError('changed AV1 configuration record')
        profile, level, flags = record[1] >> 5, record[1] & 31, record[2]
        tier = 'H' if flags & 128 else 'M'
        depth = 12 if profile == 2 and flags & 32 and flags & 64 else 10 if flags & 64 else 8
        mono, x, y = (int(bool(flags & mask)) for mask in (16, 8, 4))
        position = flags & 3 if x and y else 0
        p, t, m = (int.from_bytes(colour[i:i + 2], 'big') for i in (4, 6, 8))
        expected = (f'av01.{profile}.{level:02}{tier}.{depth:02}.{mono}.{x}{y}{position}'
                    f'.{p:02}.{t:02}.{m:02}.{int(bool(colour[10] & 128))}')
        if expected != 'av01.0.00M.08.0.110.06.06.06.1':
            raise ValueError('changed AV1 configuration or expected codec string')
    except (OSError, ValueError) as error:
        raise SystemExit(prefix + str(error)) from error
    sides = []
    args = ['-v', 'error', '-of', 'json', '-show_entries',
            'stream=codec_name,codec_tag_string,mime_codec_string', str(path)]
    for tool in (['ffprobe'], [coreav, 'ffprobe']):
        code, out, _ = run(tool + args)
        try:
            sides.append(json.loads(out)['streams'] if code == 0 else None)
        except (ValueError, KeyError, TypeError) as error:
            raise SystemExit(prefix + 'invalid stream listing') from error
    stream = {'codec_name': 'av1', 'codec_tag_string': 'av01'}
    if sides[0] == sides[1] == [dict(stream, mime_codec_string=expected)]:
        raise SystemExit(prefix + 'oracle now agrees; remove the obsolete exception')
    if sides != [[dict(stream, mime_codec_string='av01.0.00M.08')],
                 [dict(stream, mime_codec_string=expected)]]:
        raise SystemExit(prefix + 'the exact codec-string disagreement changed')
    return True


EMPTY_EDIT_FIXTURES = ('clip_audio_late.mp4', 'edit_list.mp4')


def empty_edit_padding(data):
    """The discard padding ISO/IEC 14496-12 8.6.6 gives the last packet of
    the one audio track of `data`, whose edit list is an empty edit and one
    media edit and whose last sample ends inside an AAC LC frame, and the
    one `ffprobe` 9.0.2 counts (more by the empty edit's length less the
    media time, black box). Raises ValueError for any other layout."""
    def boxes(start, end):
        while start < end:
            if start + 8 > end:
                raise ValueError('truncated box header')
            size = int.from_bytes(data[start:start + 4], 'big')
            if size < 8 or start + size > end:
                raise ValueError('changed or invalid box layout')
            yield data[start + 4:start + 8], start + 8, start + size
            start += size

    def one(start, end, kind):
        found = [(a, b) for k, a, b in boxes(start, end) if k == kind]
        if len(found) != 1:
            raise ValueError('expected exactly one ' + kind.decode('ascii') + ' box')
        return found[0]

    def u32(at):
        return int.from_bytes(data[at:at + 4], 'big')

    moov = one(0, len(data), b'moov')
    a, _ = one(*moov, b'mvhd')
    if data[a] != 0:
        raise ValueError('changed movie header version')
    movie_scale = u32(a + 12)
    audio = []
    for kind, a, b in boxes(*moov):
        mdia = one(a, b, b'mdia') if kind == b'trak' else None
        if mdia and data[one(*mdia, b'hdlr')[0] + 8:][:4] == b'soun':
            audio.append((a, b, mdia))
    if len(audio) != 1:
        raise ValueError('expected exactly one audio track')
    trak_start, trak_end, mdia = audio[0]
    a, b = one(*one(trak_start, trak_end, b'edts'), b'elst')
    if data[a] != 0 or u32(a + 4) != 2 or b - a != 8 + 2 * 12:
        raise ValueError('changed edit list')
    empty, empty_time, media, media_time = u32(a + 8), u32(a + 12), u32(a + 20), u32(a + 24)
    if empty_time != 0xFFFFFFFF or media_time >= 0x80000000 \
            or u32(a + 16) != 0x10000 or u32(a + 28) != 0x10000:
        raise ValueError('changed edit list')
    a, _ = one(*mdia, b'mdhd')
    if data[a] != 0:
        raise ValueError('changed media header version')
    scale, duration = u32(a + 12), u32(a + 16)
    stbl = one(*one(*mdia, b'minf'), b'stbl')
    a, b = one(*stbl, b'stts')
    count = u32(a + 4)
    if b - a != 8 + 8 * count or count == 0:
        raise ValueError('changed time-to-sample table')
    deltas = [(u32(a + 8 + 8 * i), u32(a + 12 + 8 * i)) for i in range(count)]
    total = sum(n * d for n, d in deltas)
    last_start = total - deltas[-1][1]
    a, b = one(*stbl, b'stsc')
    count = u32(a + 4)
    if b - a != 8 + 12 * count or count == 0 or any(u32(a + 16 + 12 * i) != 1 for i in range(count)):
        raise ValueError('changed sample descriptions')
    a, b = one(*stbl, b'stsd')
    if u32(a + 4) != 1:
        raise ValueError('changed sample descriptions')
    entry = one(a + 8, b, b'mp4a')
    a, b = one(entry[0] + 28, entry[1], b'esds')

    def descriptor(at, tag):
        """ISO/IEC 14496-1 8.3.3: the payload of the descriptor at `at`."""
        if at >= b or data[at] != tag:
            raise ValueError('changed AAC configuration')
        size, at = 0, at + 1
        for _ in range(4):
            if at >= b:
                raise ValueError('changed AAC configuration')
            size, at = size << 7 | data[at] & 0x7F, at + 1
            if not data[at - 1] & 0x80:
                break
        return at, at + size

    es, _ = descriptor(a + 4, 3)
    flags = data[es + 2]
    at = es + 3 + (2 if flags & 0x80 else 0) + (2 if flags & 0x20 else 0)
    if flags & 0x40:
        at += 1 + data[at]
    config, _ = descriptor(at, 4)
    a, b = descriptor(config + 13, 5)
    # AudioSpecificConfig (ISO/IEC 14496-3 1.6.2.1, 4.4.1): AAC LC, its
    # sampling frequency, channels, then frameLengthFlag, dependsOnCoreCoder
    # and extensionFlag; then the backward-compatible extension that rules
    # SBR out (0x2b7, object type 5, sbrPresentFlag 0), without which
    # implicit SBR is open at any core rate (1.6.5.3), and nothing else.
    if b - a != 5 or data[a + 2:b] != b'\x56\xe5\x00':
        raise ValueError('changed AAC configuration')
    specific = int.from_bytes(data[a:a + 2], 'big')
    rates = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350]
    index = specific >> 7 & 15
    if specific >> 11 != 2 or index >= len(rates) or rates[index] != scale or specific & 3 != 0:
        raise ValueError('changed AAC configuration')
    frame = 960 if specific & 4 else 1024
    edit_end = media_time + (media * scale + movie_scale // 2) // movie_scale
    end = min(edit_end, duration, total)
    padding = last_start + frame - end
    oracle = padding + (empty * scale + movie_scale // 2) // movie_scale - media_time
    if not 0 < padding < frame or oracle <= frame or media_time > last_start:
        raise ValueError('changed sample timing')
    return padding


@functools.cache
def empty_edit_padding_fixture(coreav, path):
    """Validate an empty-edit fixture's bytes and its exact oracle
    disagreement: only the last audio packet's Skip Samples side data
    differs, CoreAV's with the padding the bytes give and ffprobe's absent
    (KD-FF9-EMPTY-EDIT-PADDING). An oracle that starts agreeing must
    retire this exception."""
    if path.name not in EMPTY_EDIT_FIXTURES:
        return False
    prefix = 'KD-FF9-EMPTY-EDIT-PADDING: '
    try:
        if path.stat().st_size > 64 * 1024 * 1024:
            raise ValueError('fixture exceeds the evidence reader limit')
        padding = empty_edit_padding(path.read_bytes())
    except (OSError, ValueError) as error:
        raise SystemExit(prefix + str(error)) from error
    pairs = (canonical(coreav, path) or {}).get('packets.side_data_list[Skip Samples]')
    if not pairs:
        return False
    expected = json.dumps({'discard_padding': padding, 'discard_reason': 0, 'side_data_type': 'Skip Samples',
                           'skip_reason': 0, 'skip_samples': 0}, sort_keys=True)
    def tail(context):
        return context.get('codec_type') == 'audio' and context.get('last')
    last = [(f, c) for f, c, context in pairs if tail(context)]
    if last == [(expected, expected)]:
        raise SystemExit(prefix + 'oracle now agrees; remove the obsolete exception')
    return last == [('N/A', expected)] and all(f == c for f, c, context in pairs if not tail(context))


CANONICAL_ARGS = ['-v', 'error', '-of', 'json', '-show_format', '-show_streams', '-show_programs',
                  '-show_packets']


def listed_values(doc):
    """[(section, {key: value}, context)] for the format, each stream, each
    program's streams and each packet of a JSON listing, a packet's side
    data under `side_data_list[<type>]`. A stream's and a packet's context
    name its stream's codec; a
    packet's also its position in the listing, whether it is its stream's
    last, and whether it carries Skip Samples side data."""
    def leaves(node, prefix, flat):
        for k, v in node.items():
            if isinstance(v, dict):
                leaves(v, f'{prefix}{k}.', flat)
            elif not isinstance(v, list):
                flat[prefix + k] = str(v)
        return flat

    def codec(stream):
        return {'codec_type': stream.get('codec_type'), 'codec_name': stream.get('codec_name')}

    streams = {s.get('index'): s for s in doc.get('streams', [])}
    items = [('format', leaves(doc.get('format', {}), '', {}), {})]
    items += [('streams', leaves(s, '', {}), codec(s)) for s in doc.get('streams', [])]
    items += [('programs.streams', leaves(s, '', {}), codec(s))
              for program in doc.get('programs', []) for s in program.get('streams', [])]
    packets = doc.get('packets', [])
    last = {p.get('stream_index'): i for i, p in enumerate(packets)}
    for i, packet in enumerate(packets):
        index = packet.get('stream_index')
        skip = any(s.get('side_data_type') == 'Skip Samples' for s in packet.get('side_data_list', []))
        context = {**codec(streams.get(index, {})), 'last': last[index] == i, 'skip': skip, 'position': i}
        flat = leaves(packet, '', {})
        for side in packet.get('side_data_list', []):
            key = f"side_data_list[{side.get('side_data_type', '')}]"
            flat[key] = flat.get(key, '') + json.dumps(side, sort_keys=True)
        items.append(('packets', flat, context))
    return items


@functools.cache
def canonical(coreav, path):
    """{key: [(ffprobe's value, coreav's, context)], or None where the two
    list a different number of items} over the whole file, a value one
    tool leaves out N/A (the JSON writer does not print an N/A), the context
    ffprobe's; None when either tool fails."""
    sides = []
    for tool in (['ffprobe'], [coreav, 'ffprobe']):
        code, out, _ = run(tool + CANONICAL_ARGS + [str(path)])
        try:
            sides.append(listed_values(json.loads(out)) if code == 0 else None)
        except ValueError:
            return None
        if sides[-1] is None:
            return None
    names = collections.defaultdict(dict)
    for section, flat, _ in sides[0] + sides[1]:
        names[section].update(dict.fromkeys(flat))
    ref, got = ([(f'{section}.{k}', flat.get(k, 'N/A'), context)
                 for section, flat, context in side for k in names[section]] for side in sides)
    if [k for k, _, _ in ref] != [k for k, _, _ in got]:
        return {}
    out = collections.defaultdict(list)
    for (key, f, context), (_, c, _) in zip(ref, got):
        out[key].append((f, c, context))
    return out


def number(text):
    try:
        return fractions.Fraction(text)
    except (ValueError, ZeroDivisionError):
        return None


PREFIXES = {'': 1, 'K': 1000, 'M': 1000 ** 2, 'G': 1000 ** 3, 'Ki': 1024, 'Mi': 1024 ** 2, 'Gi': 1024 ** 3}


def plain(value):
    """A value as -pretty, -sexagesimal, -unit or -prefix leave it or not:
    a number, None for N/A, unknown or nothing, else the text."""
    text = value.strip()
    if text in ('', 'N/A', 'unknown', '[]'):
        return None
    clock = re.fullmatch(r'(-?)(\d+):(\d\d):(-?[\d.]+)', text)
    if clock:
        sign = -1 if clock[1] else 1
        return sign * (int(clock[2]) * 3600 + int(clock[3]) * 60 + fractions.Fraction(clock[4]))
    unit = re.fullmatch(r'(-?[\d.]+) ?(Ki|Mi|Gi|K|M|G)?(bit/s|byte|B|s|Hz)', text)
    if unit:
        return fractions.Fraction(unit[1]) * PREFIXES[unit[2] or '']
    return number(text) if number(text) is not None else text


RELATIONS = {
    'ffprobe greater': lambda f, c: f > c,
    'ffprobe smaller': lambda f, c: f < c,
    'ffprobe double': lambda f, c: f == 2 * c,
}


def satisfies(rule, f, c, context):
    """Whether one pair of differing values over the whole file, ffprobe's
    `f` and coreav's `c` of one stream or packet, is the difference
    `rule` accepts; a list of rules accepts what any of them does."""
    if isinstance(rule, list):
        return any(satisfies(r, f, c, context) for r in rule)
    where = rule.get('where', {})
    if any(context.get(k) != where[k] for k in ('codec_type', 'codec_name') if k in where):
        return False
    if where.get('packet') == 'last' and not context.get('last'):
        return False
    if where.get('packet') == 'skip samples' and not context.get('skip'):
        return False
    if f != rule.get('ffprobe', f) or c != rule.get('coreav', c):
        return False
    if 'pairs' in rule and [f, c] not in rule['pairs']:
        return False
    if 'at' in rule and [context.get('position'), f, c] not in rule['at']:
        return False
    if rule.get('relation') == 'ffprobe joins coreav':
        return ';' in f and c in f.split(';')
    if 'relation' in rule or 'difference' in rule or 'ratio' in rule:
        x, y = number(f), number(c)
        if x is None or y is None:
            return False
        if 'difference' in rule and x - y not in rule['difference']:
            return False
        if 'ratio' in rule and not (y and number(rule['ratio'][0]) <= x / y <= number(rule['ratio'][1])):
            return False
        return 'relation' not in rule or RELATIONS[rule['relation']](x, y)
    return True


def accepts(coreav, path, key, rule, ref_values, values):
    """Whether `rule` accepts every difference of one key of one command's
    two outputs (ffprobe's values `ref_values`, coreav's `values`): a
    key one tool alone prints, side data CoreAV lists on only some of the
    packets ffprobe lists it on, the same on each ([canonical]), or values
    that differ only where they differ over the whole file, each
    difference there the one the rule names."""
    if isinstance(rule, dict) and rule.get('evidence') == 'av1 nclx':
        if key != 'streams.mime_codec_string' or not av1_nclx_fixture(coreav, path):
            return False
    if isinstance(rule, dict) and rule.get('evidence') == 'empty edit padding':
        if not empty_edit_padding_fixture(coreav, path):
            return False
    only = rule.get('only') if isinstance(rule, dict) else None
    if only == 'ffprobe':
        return all(plain(v) is None for v in values)
    if only == 'coreav':
        return all(plain(v) is None for v in ref_values)
    if only == 'part of ffprobe':
        if collections.Counter(values) - collections.Counter(ref_values):
            return False
        if key.endswith('[]'):
            return True
        pairs = (canonical(coreav, path) or {}).get(key)
        return pairs is not None and all(c in ('N/A', f) for f, c, _ in pairs)
    pairs = (canonical(coreav, path) or {}).get(key)
    if pairs is None:
        return False
    if isinstance(rule, dict) and 'packets' in rule:
        rule = {'at': rule['packets'].get(path.name, [])}
    differ = [(f, c, context) for f, c, context in pairs if f != c]
    if not all(satisfies(rule, f, c, context) for f, c, context in differ):
        return False
    allowed = {(plain(f), plain(c)) for f, c, _ in differ}
    got = [(plain(f), plain(c)) for f, c in zip(ref_values, values)]
    if len(ref_values) == len(values):
        return all(f == c or (f, c) in allowed for f, c in got)
    # The JSON writer leaves an N/A out, so the items no longer pair.
    ref_only = collections.Counter(map(plain, ref_values)) - collections.Counter(map(plain, values))
    only_here = collections.Counter(map(plain, values)) - collections.Counter(map(plain, ref_values))
    return (not ref_only - collections.Counter(f for f, _ in allowed)
            and not only_here - collections.Counter(c for _, c in allowed))


def writer_of(args):
    """The writer a command prints with, and its options, defaults included."""
    spec = 'default'
    for flag, value in zip(args, args[1:]):
        if flag in WRITER_FLAGS:
            spec = value
    name, _, rest = spec.partition('=')
    options = dict(WRITER_DEFAULTS.get(name, {}))
    for item in filter(None, rest.split(':')):
        k, _, v = item.partition('=')
        options[WRITER_OPTIONS.get(k, k)] = v
    return name, options


def on(options, name):
    return options.get(name, '0') not in ('0', '')


def full_args(args, name, options):
    """`args` with the writer set to show every key, section name and
    wrapper, or None when it already does."""
    full = {**options, **WRITER_FULL[name]}
    if full == options:
        return None
    spec = name + '=' + ':'.join(f'{k}={v}' for k, v in full.items())
    return [spec if i and args[i - 1] in WRITER_FLAGS else a for i, a in enumerate(args)]


def records(text, escape):
    """(start, line) for each line of a compact or CSV output; a CSV value in
    quotes may hold line ends."""
    out, start, quoted = [], 0, False
    for i, ch in enumerate(text):
        if escape == 'csv' and ch == '"':
            quoted = not quoted
        elif ch == '\n' and not quoted:
            out.append((start, text[start:i]))
            start = i + 1
    return out + [(start, text[start:])]


def split_items(line, sep, escape):
    """A compact or CSV line cut at each separator that is not escaped."""
    items, start, quoted, i = [], 0, False, 0
    while i < len(line):
        ch = line[i]
        if escape == 'c' and ch == '\\':
            i += 2
            continue
        if escape == 'csv' and ch == '"':
            quoted = not quoted
        elif ch == sep and not quoted:
            items.append(line[start:i])
            start = i + 1
        i += 1
    items.append(line[start:])
    return items


def derive(text, name, options):
    """What a writer prints under `options`, from what it prints under
    WRITER_FULL."""
    lines = text.split('\n')
    if name == 'default':
        if on(options, 'nw'):
            lines = [x for x in lines if not (x.startswith('[') and x.endswith(']'))]
        if on(options, 'nk'):
            lines = [x.split('=', 1)[-1] for x in lines]
    elif name in ('compact', 'csv'):
        out = []
        for _, x in records(text, options['e']):
            items = split_items(x, options['s'], options['e']) if x else []
            while items and not on(options, 'p') and '=' not in items[0]:
                items = items[1:]
            if on(options, 'nk'):
                items = [i.split('=', 1)[-1] for i in items]
            out.append(options['s'].join(items))
        lines = out
    elif name == 'flat':
        lines = [x.partition('=')[0].replace('.', options['s']) + ''.join(x.partition('=')[1:])
                 for x in lines]
    return '\n'.join(lines)


def section_key(name):
    """`STREAM` -> `streams`, `FORMAT` -> `format`: a section as
    `key_name` names it."""
    name = name.lower()
    return name if name == 'format' else name + 's'


def leaf_key(path, key):
    for prefix, sub in (('TAG:', 'tags.'), ('DISPOSITION:', 'disposition.'),
                        ('tag:', 'tags.'), ('disposition:', 'disposition.')):
        if key.startswith(prefix):
            key = sub + key[len(prefix):]
    return '.'.join(path + [key])


# A stream group (FFmpeg 9.0.2 makes one of a QuickTime timecode track) is
# one item, `stream_groups`, in every writer.
def side_data_key(path, kind):
    """One side data entry, by its type: `packets.side_data_list[Skip Samples]`."""
    return '.'.join(path + ['side_data_list']) + f'[{kind}]'


def parse_default(text):
    """[(key, start, end)]: each line of a key, and each side data section,
    of the default writer."""
    spans, path, group, opaque, pos = [], [], None, None, 0
    for line in text.split('\n'):
        end = pos + len(line) + 1
        if opaque is not None:
            if line == '[/STREAM_GROUP]':
                spans.append(('stream_groups', opaque, end))
                opaque = None
        elif line == '[STREAM_GROUP]':
            opaque = pos
        elif line.startswith('[') and line.endswith(']'):
            close, section = line.startswith('[/'), line.strip('[/]')
            if section == 'SIDE_DATA':
                if close and group:
                    spans.append((side_data_key(path, group[1]), group[0], end))
                group = None if close else [pos, '']
            elif close:
                path = path[:-1]
            else:
                path = path + [section_key(section)]
        elif group is not None:
            if line.startswith('side_data_type='):
                group[1] = line.split('=', 1)[1]
        elif '=' in line:
            spans.append((leaf_key(path, line.split('=', 1)[0]), pos, end))
        pos = end
    return spans


def parse_compact(text, options):
    """[(key, start, end)]: each item of a key with the separator before it,
    and each side data entry, of the compact and CSV writers."""
    spans, sep, opaque = [], options['s'], None
    for pos, line in records(text, options['e']):
        items = split_items(line, sep, options['e'])
        if opaque is not None or items[0] == 'stream_group':
            opaque = pos if opaque is None else opaque
            if not line:
                spans.append(('stream_groups', opaque, pos + 1))
                opaque = None
            continue
        path = [section_key(items[0])] if items[0] else []
        at, groups = pos + len(items[0]), {}
        for item in items[1:]:
            start, at = at, at + len(sep) + len(item)
            key, eq, value = item.partition('=')
            prefix, colon, rest = key.partition(':')
            if not eq:
                groups[start] = [start, at, '']
            elif colon and prefix.startswith('side_datum/'):
                g = groups.setdefault(prefix, [start, at, ''])
                g[1] = at
                if rest == 'side_data_type':
                    g[2] = value
            else:
                spans.append((leaf_key(path, key), start, at))
        spans += [(side_data_key(path, kind), s, e) for s, e, kind in groups.values()]
    return spans


def parse_flat(text):
    """[(key, start, end)]: each line of a key of the flat writer, and the
    lines of each side data entry together."""
    spans, groups, pos = [], {}, 0
    for line in text.split('\n'):
        end = pos + len(line) + 1
        name, eq, value = line.partition('=')
        parts, path, i, group = name.split('.'), [], 0, None
        while i < len(parts):
            if i + 2 < len(parts) and parts[i + 2].isdigit():
                if parts[i] == 'side_data_list':
                    group = '.'.join(parts[:i + 3])
                    break
                path.append(parts[i])
                i += 3
            else:
                path.append(parts[i])
                i += 1
        if parts[0] == 'stream_groups':
            g = groups.setdefault('stream_groups', [None, pos, end, ''])
            g[2] = end
        elif group:
            g = groups.setdefault(group, [path, pos, end, ''])
            g[2] = end
            if parts[-1] == 'side_data_type':
                g[3] = value.strip('"')
        elif eq:
            spans.append(('.'.join(path), pos, end))
        pos = end
    return spans + [(side_data_key(p, kind) if p is not None else 'stream_groups', s, e)
                    for p, s, e, kind in groups.values()]


class JsonNode:
    def __init__(self, key, start):
        self.key, self.start, self.end = key, start, start
        self.open, self.children, self.value = None, [], None


def parse_json_text(text):
    """The JSON writer's output as a tree; a member's span covers its name
    and its value."""
    pos = 0

    def ws():
        nonlocal pos
        while pos < len(text) and text[pos] in ' \t\r\n':
            pos += 1

    def string():
        nonlocal pos
        start = pos
        pos += 1
        while text[pos] != '"':
            pos += 2 if text[pos] == '\\' else 1
        pos += 1
        return json.loads(text[start:pos])

    def value(node):
        nonlocal pos
        ch = text[pos]
        if ch in '{[':
            node.open, close = ch, '}' if ch == '{' else ']'
            pos += 1
            ws()
            while text[pos] != close:
                child = JsonNode(node.key, pos)
                if ch == '{':
                    name = string()
                    ws()
                    pos += 1
                    ws()
                    child.key = f'{node.key}.{name}' if node.key else name
                value(child)
                child.end = pos
                node.children.append(child)
                ws()
                if text[pos] == ',':
                    pos += 1
                    ws()
            pos += 1
        elif ch == '"':
            node.value = string()
        else:
            start = pos
            while pos < len(text) and text[pos] not in ',}] \t\r\n':
                pos += 1
            node.value = text[start:pos]

    root = JsonNode('', 0)
    ws()
    if pos < len(text):
        value(root)
    root.start, root.end = 0, len(text)
    return root


def json_side_data(node):
    """The side data key of an entry of a `side_data_list`."""
    kind = next((c.value for c in node.children if c.key.endswith('.side_data_type')), '')
    base = node.key[:-len('side_data_list')].rstrip('.')
    return side_data_key([base] if base else [], kind)


def parse_json(text):
    """[(key, start, end)]: each leaf member, and each side data entry, of
    the JSON writer."""
    spans = []

    def walk(node, in_list):
        for child in node.children:
            if in_list:
                spans.append((json_side_data(child), child.start, child.end))
            elif child.key == 'stream_groups':
                spans.append((child.key, child.start, child.end))
            elif child.open:
                walk(child, child.open == '[' and child.key.endswith('side_data_list'))
            else:
                spans.append((child.key, child.start, child.end))
    walk(parse_json_text(text), False)
    return spans


def json_without(text, keys):
    """The JSON writer's output with the members of `keys` taken out, the
    separators between those left kept, and a container left empty taken
    out too."""
    def render(node, in_list):
        if in_list:
            return None if json_side_data(node) in keys else text[node.start:node.end]
        if not node.open or node.key == 'stream_groups':
            return None if node.key in keys else text[node.start:node.end]
        kids = node.children
        if not kids:
            return text[node.start:node.end]
        inner = node.open == '[' and node.key.endswith('side_data_list')
        kept = [(i, r) for i, r in ((i, render(c, inner)) for i, c in enumerate(kids)) if r is not None]
        if not kept:
            return None
        out = [text[node.start:kids[0].start]]
        for j, (i, r) in enumerate(kept):
            if j:
                out.append(text[kids[i - 1].end:kids[i].start])
            out.append(r)
        out.append(text[kids[-1].end:node.end])
        return ''.join(out)
    return render(parse_json_text(text), False) or ''


def item_spans(text, name, options):
    if name == 'default':
        return parse_default(text)
    if name in ('compact', 'csv'):
        return parse_compact(text, options)
    if name == 'flat':
        return parse_flat(text)
    return parse_json(text)


def without(text, name, options, keys):
    """`text` with every item of `keys` taken out."""
    if name == 'json':
        return json_without(text, keys)
    out, at = [], 0
    for key, start, end in sorted(item_spans(text, name, options), key=lambda s: s[1]):
        if key in keys:
            out.append(text[at:start])
            at = end
    return ''.join(out) + text[at:]


def item_value(text, name, options):
    """The value of one item of a key, as `item_spans` cuts it."""
    if name == 'json':
        value = text.partition(':')[2].strip()
        try:
            return str(json.loads(value))
        except ValueError:
            return value
    value = text.rstrip('\n').partition('=')[2]
    if name == 'flat' and value.startswith('"'):
        return value[1:-1]
    if name == 'csv' and value.startswith('"'):
        return value[1:-1].replace('""', '"')
    return value


def item_values(text, name, options):
    out = collections.defaultdict(list)
    for key, start, end in sorted(item_spans(text, name, options), key=lambda s: s[1]):
        out[key].append(item_value(text[start:end], name, options))
    return out


def differing_keys(a, b, name, options):
    """The keys whose items differ between two outputs of one writer."""
    def items(text):
        out = collections.defaultdict(list)
        for key, start, end in item_spans(text, name, options):
            out[key].append(text[start:end].strip(' ,\n' + options.get('s', '')))
        return out
    x, y = items(a), items(b)
    return sorted(k for k in set(x) | set(y) if x.get(k) != y.get(k))


def full_outputs(coreav, args, path, ref_out, out):
    """(writer, options, [ffprobe's output, coreav's]) with every key, section
    name and wrapper shown, or None when the writer is not one of
    WRITER_FULL, either tool's output under them does not give back what
    it printed, or the JSON writer printed something that is not JSON."""
    name, options = writer_of(args)
    if name not in WRITER_FULL:
        return None
    printed = [ref_out.decode('utf-8', 'replace'), out.decode('utf-8', 'replace')]
    if name == 'json':
        try:
            for text in printed:
                json.loads(text)
        except ValueError:
            return None
    full = full_args(args, name, options)
    if full is None:
        return name, options, printed
    texts = []
    for tool, text in zip((['ffprobe'], [coreav, 'ffprobe']), printed):
        code, again, _ = run(tool + substitute(full, path))
        again = again.decode('utf-8', 'replace')
        if code != 0 or derive(again, name, options) != text:
            return None
        texts.append(again)
    return name, {**options, **WRITER_FULL[name]}, texts


def ffmpeg9_covers(coreav, args, path, ref_out, out):
    """For two outputs of one command on one file that differ: the
    ffmpeg9.json entries that explain every difference, or None, and the
    keys that differ, or None when the writer's output cannot be read."""
    found = full_outputs(coreav, args, path, ref_out, out)
    if found is None:
        return None, None
    name, options, (a, b) = found
    keys = differing_keys(a, b, name, options)
    allowed = ffmpeg9().get(path.name, {})
    if not allowed or without(a, name, options, allowed) != without(b, name, options, allowed):
        return None, keys
    ref_values, values = item_values(a, name, options), item_values(b, name, options)
    for k in keys:
        if k in allowed and not accepts(coreav, path, k, ffmpeg9_rules()[allowed[k]][k], ref_values[k], values[k]):
            return None, keys
    return sorted({allowed[k] for k in keys if k in allowed}) or None, keys


SECTION_ARGS = ['-v', 'error', '-of', 'json', '-show_format', '-show_streams',
                '-show_chapters', '-show_programs']
PACKET_ARGS = ['-v', 'error', '-of', 'compact', '-count_packets', '-show_packets']


def flatten(doc):
    """(key path, value) for every leaf; array elements keep their index,
    so streams pair by index."""
    out = {}

    def walk(prefix, node):
        if isinstance(node, dict):
            for k, v in node.items():
                walk(f'{prefix}.{k}' if prefix else k, v)
        elif isinstance(node, list):
            for i, v in enumerate(node):
                walk(f'{prefix}[{i}]', v)
        else:
            out[prefix] = node
    walk('', doc)
    return out


def key_name(path):
    """`streams[1].tags.language` -> `streams.tags.language`."""
    return '.'.join(part.split('[', 1)[0] for part in path.split('.'))


def packet_fields(line):
    """A compact packet line as {key: value}; side data keys under
    `side_data_list.`."""
    out = {}
    for item in line.rstrip('\n').split('|')[1:]:
        if '=' not in item:
            continue
        k, v = item.split('=', 1)
        if ':' in k:
            k = 'side_data_list.' + k.split(':', 1)[1]
        out[k] = v
    return out


def run_to_file(cmd, path):
    with open(path, 'wb') as f:
        try:
            return subprocess.run(cmd, stdout=f, stderr=subprocess.DEVNULL, timeout=TIMEOUT,
                                  env=ENV).returncode
        except subprocess.TimeoutExpired:
            return None


def compare_packets(coreav, path):
    """Packets paired by position, read line by line from both listings, so
    that a long file needs no memory."""
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        ref_path, got_path = pathlib.Path(tmp) / 'ref', pathlib.Path(tmp) / 'got'
        if run_to_file(['ffprobe'] + PACKET_ARGS + [str(path)], ref_path) != 0:
            return None
        code = run_to_file([coreav, 'ffprobe'] + PACKET_ARGS + [str(path)], got_path)
        result, examples = {}, {}
        with open(ref_path, errors='replace') as ref, open(got_path, errors='replace') as got:
            n = 0
            for line in ref:
                other = got.readline() if code == 0 else ''
                a, b = packet_fields(line), packet_fields(other)
                for k, v in a.items():
                    name = 'packets.' + k
                    ok = b.get(k) == v
                    result[name] = result.get(name, True) and ok
                    if not ok and name not in examples:
                        examples[name] = (f'packets[{n}].{k}', v, b.get(k))
                n += 1
        return result, examples, code


def compare_file(coreav, path, args):
    if args is PACKET_ARGS:
        return compare_packets(coreav, path)
    ref_code, ref_out, _ = run(['ffprobe'] + args + [str(path)])
    if ref_code != 0:
        return None
    code, out, err = run([coreav, 'ffprobe'] + args + [str(path)])
    try:
        ref = flatten(json.loads(ref_out))
    except ValueError:
        return None
    try:
        got = flatten(json.loads(out)) if code == 0 else {}
    except ValueError:
        got = {}
    result = {}
    examples = {}
    for k, v in ref.items():
        name = key_name(k)
        ok = got.get(k) == v
        result[name] = result.get(name, True) and ok
        if not ok:
            examples.setdefault(name, (k, v, got.get(k)))
    return result, examples, code


STRICT = 'KD-STRICT (docs/known-divergences.md)'


def strict_rejection(coreav, path):
    """Whether KD-STRICT covers CoreAV rejecting `path`: `ffprobe` reads the
    file as ISO base media (`format_name` `mov,mp4,...`, whatever its
    extension), `coreav probe` names the kind of defect (truncated or
    invalid structure, a limit), and the file's bytes independently show
    that kind, read by the walk of `scripts/oracle_kd.py` that the library
    oracle uses for the same record. A valid file CoreAV rejects is never
    covered, nor a file of another container, whose first bytes the box
    walk would read as a box past the end."""
    code, out, _ = run(['ffprobe', '-v', 'error', '-of', 'json', '-show_entries',
                        'format=format_name', str(path)])
    try:
        name = json.loads(out)['format']['format_name'] if code == 0 else ''
    except (ValueError, KeyError, TypeError):
        return False
    if not name.startswith('mov,mp4'):
        return False
    _, _, err = run([coreav, 'probe', str(path)])
    try:
        kind = json.loads(err).get('kind')
    except (ValueError, AttributeError):
        return False
    kd = oracle_kd()
    evidence = kd.EVIDENCE.get(kind, set())
    return bool(evidence and evidence & kd.media_defects(path.parent, path.name))


@functools.cache
def oracle_kd():
    """`scripts/oracle_kd.py`, from the repository or beside this file."""
    for d in (HERE.parent.parent / 'scripts', HERE):
        if (d / 'oracle_kd.py').exists():
            spec = importlib.util.spec_from_file_location('oracle_kd', d / 'oracle_kd.py')
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            return module
    raise FileNotFoundError('oracle_kd.py')


def compare_all(coreav, path):
    """Both listings of `path` compared key by key: (keys, examples, failed,
    record, covered), or None when ffprobe reads neither. `keys` maps each key
    ffprobe prints to whether every occurrence agrees. When `coreav
    ffprobe` rejects the file (exit 1, its status for invalid input) and a
    known divergence covers the rejection, `record` names it and the keys
    of the listings it rejected (`covered`) agree through it (they stay in
    the denominator); else `record` is None and `covered` empty. `failed`:
    a listing exited other than 0 that `record` does not cover."""
    keys, examples, other, rejected = {}, {}, False, set()
    for args in (SECTION_ARGS, PACKET_ARGS):
        r = compare_file(coreav, path, args)
        if r is None:
            continue
        result, ex, code = r
        if code == 1:
            rejected |= set(result)
        elif code != 0:
            other = True
        for name, ok in result.items():
            keys[name] = keys.get(name, True) and ok
        for name, e in ex.items():
            examples.setdefault(name, e)
    if not keys:
        return None
    record = STRICT if rejected and strict_rejection(coreav, path) else None
    covered = rejected if record else set()
    for name in covered:
        keys[name] = True
        examples.pop(name, None)
    return keys, examples, other or bool(rejected and not record), record, covered


def cmd_keys(opts):
    files = media_files(opts.media)
    if opts.list:
        files = [pathlib.Path(line.strip()) for line in open(opts.list) if line.strip()]
    seen = collections.Counter()
    agree = collections.Counter()
    covered = collections.Counter()
    examples = collections.defaultdict(list)
    rejected = []
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        results = list(pool.map(lambda path: (path, compare_all(opts.coreav, path)), files))
    for path, r in results:
        if r is None:
            continue
        result, ex, failed, record, names = r
        if failed or record:
            rejected.append({'file': str(path), 'covered_by': None if failed else record})
        for name, ok in result.items():
            seen[name] += 1
            agree[name] += ok
            covered[name] += name in names
        for name, e in ex.items():
            if len(examples[name]) < 5:
                examples[name].append({'file': path.name, 'path': e[0],
                                       'ffprobe': e[1], 'coreav': e[2]})
    rows = []
    for name in sorted(seen, key=lambda n: (agree[n] / seen[n], n)):
        rows.append({'key': name, 'files': seen[name], 'agree': agree[name],
                     'via_known_divergence': covered[name],
                     'rate': round(100.0 * agree[name] / seen[name], 2)})
    report = {'files': len(files), 'rejected': rejected, 'keys': rows,
              'examples': examples}
    if opts.report:
        pathlib.Path(opts.report).write_text(json.dumps(report, indent=1))
    # A key is below the bar on its unrounded share, as the measurement
    # record counts it: 97.998% of files misses a bar of 98, where the
    # rounded `rate` of the tables reads 98.0.
    below = [r for r in rows if r['agree'] * 100.0 < opts.bar * r['files']]
    uncovered = [r for r in rejected if not r['covered_by']]
    print(json.dumps({'files': len(files), 'keys': len(rows), 'below_bar': len(below),
                      'rejected': len(rejected), 'rejected_uncovered': len(uncovered)}))
    for r in below[:opts.top]:
        print(f"{r['rate']:6.2f}% {r['agree']}/{r['files']} {r['key']}")
    return 1 if below or uncovered else 0


def cmd_commands_010(opts):
    """R1: every `ffprobe` command of commands-0.1.0 on every file of its
    families, counted per (command, family). With no baseline, a difference
    or a failure of `coreav` on a file ffprobe reads misses the bar."""
    if opts.baseline or opts.write_baseline:
        raise SystemExit('R1 has no baseline: --set 0.1.0 takes no --baseline or --write-baseline')
    files = [p for p in media_files(opts.media) if release_round.family(p)]
    for path in files:
        av1_nclx_fixture(opts.coreav, path)
        empty_edit_padding_fixture(opts.coreav, path)
    aside = {p.name: covered_by_record(opts.coreav, p) for p in files if p.name in SET_ASIDE}
    counts = collections.defaultdict(collections.Counter)
    set_aside = collections.Counter()
    failures = []

    def both(args, path):
        argv = substitute(args, path)
        ref_code, ref_out, _ = run(['ffprobe'] + argv)
        if ref_code != 0:
            return path, ref_code, ref_out, None, b'', b'', None
        code, out, err = run([opts.coreav, 'ffprobe'] + argv)
        entries = None
        if code == 0 and out != ref_out and not aside.get(path.name) and path.name in ffmpeg9():
            entries, _ = ffmpeg9_covers(opts.coreav, args, path, ref_out, out)
        return path, ref_code, ref_out, code, out, err, entries

    strict = {}

    def strictly_rejected(path):
        if path.name not in strict:
            strict[path.name] = strict_rejection(opts.coreav, path)
        return strict[path.name]

    for entry, n, command in release_round.commands('ffprobe'):
        args = command['args']
        families = release_round.containers(entry, command)
        targets = [p for p in files if release_round.family(p) in families]
        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            results = list(pool.map(lambda p: both(args, p), targets))
        for path, ref_code, ref_out, code, out, err, entries in results:
            c = counts[(entry['id'], n, release_round.family(path))]
            if ref_code != 0:
                c['ffprobe fails'] += 1
            elif code == 1 and strictly_rejected(path):
                c['set aside'] += 1
                set_aside[STRICT] += 1
            elif code != 0:
                c['coreav fails'] += 1
                failures.append({'id': entry['id'], 'command': n, 'args': args, 'file': path.name,
                                 'exit': code, 'stderr': err[:300].decode('utf-8', 'replace')})
            elif out == ref_out:
                c['identical'] += 1
            elif aside.get(path.name):
                c['set aside'] += 1
                set_aside[aside[path.name]] += 1
            elif entries:
                c['set aside'] += 1
                set_aside.update(entries)
            else:
                c['different'] += 1
                failures.append({'id': entry['id'], 'command': n, 'args': args, 'file': path.name,
                                 'diff': first_difference(ref_out, out)})
    rows, families = [], collections.defaultdict(collections.Counter)
    for (id_, n, fam), c in sorted(counts.items()):
        rows.append({'id': id_, 'command': n, 'family': fam, **c})
        families[fam].update(c)
    missed = [f"{r['id']}.{r['command']} {r['family']}" for r in rows
              if r.get('different', 0) or r.get('coreav fails', 0)]
    unmeasured = [f"{r['id']}.{r['command']} {r['family']}" for r in rows
                  if not any(r.get(k, 0) for k in ('identical', 'different', 'set aside', 'coreav fails'))]
    verdict = release_round.fallback_verdict(ENV['COREAV_FALLBACK_LOG'])
    report = {'set': '0.1.0', 'bar': 'R1',
              'files': dict(collections.Counter(release_round.family(p) for p in files)),
              'families': families, 'commands': rows, 'missed': missed, 'unmeasured': unmeasured,
              'set aside': dict(set_aside), 'fallback_log': verdict, 'failures': failures}
    if opts.report:
        pathlib.Path(opts.report).write_text(json.dumps(report, indent=1) + '\n')
    print(json.dumps({k: report[k] for k in ('files', 'families', 'set aside', 'fallback_log')}))
    for line in missed[:opts.top]:
        print(f'MISSED {line}')
    print(f'{len(missed)} (command, family) pairs miss R1')
    for line in unmeasured[:opts.top]:
        print(f'UNMEASURED {line}')
    if verdict['void']:
        print('VOID: the fallback log holds a line')
        return 2
    if len(unmeasured) == len(rows):
        print('UNMEASURED: no file of the set that ffprobe reads')
        return release_round.UNMEASURED
    if unmeasured:
        print(f'{len(unmeasured)} (command, family) pairs have no file ffprobe reads: they miss R1')
    return 1 if missed or unmeasured else 0


def key_record(path, result, patterns):
    """One file of `keys --set 0.1.0`, the set's keys alone: its family,
    whether each key agrees, the keys a known divergence covered, how
    `coreav ffprobe` rejected it if it did, and the first disagreement of
    each key."""
    record = {'file': path.name, 'family': release_round.family(path)}
    if result is None:
        return {**record, 'ffprobe_failed': True}
    keys, examples, failed, kd, covered = result
    record['keys'] = {k: ok for k, ok in keys.items() if release_round.in_set(k, patterns)}
    record['via_known_divergence'] = sorted(k for k in covered if k in record['keys'])
    record['rejected'] = 'uncovered' if failed else kd
    record['examples'] = {k: list(e) for k, e in examples.items() if k in record['keys']}
    return record


TAGS_POOLED = '(tag keys in fewer than 5 files, together)'


def keys_counts(records):
    """The counts R2 is judged on, from per-file records: per family, the
    files by outcome and, per key, [files it is in, files where it agrees,
    files where a known divergence covered it]. They name no file, so the
    counts of several runs add up (`keys-report --summaries`)."""
    out = {'fallback_log': {'lines': 0, 'refused': 0, 'void': False}, 'families': {}}
    for r in records:
        if 'fallback_log' in r:
            merge_log(out['fallback_log'], r['fallback_log'])
            continue
        if not r.get('family'):
            continue
        fam = out['families'].setdefault(r['family'], {'files': {}, 'keys': {}})
        if r.get('ffprobe_failed'):
            outcome = 'ffprobe failed'
        elif r['rejected']:
            outcome = 'rejected, uncovered' if r['rejected'] == 'uncovered' else 'rejected, covered'
        else:
            outcome = 'compared'
        fam['files'][outcome] = fam['files'].get(outcome, 0) + 1
        for k, ok in r.get('keys', {}).items():
            c = fam['keys'].setdefault(k, [0, 0, 0])
            c[0] += 1
            c[1] += ok
            c[2] += k in r['via_known_divergence']
    return out


def merge_log(into, log):
    into['lines'] += log['lines']
    into['refused'] += log['refused']
    into['void'] = into['void'] or log['void']


def merge_counts(parts):
    out = {'fallback_log': {'lines': 0, 'refused': 0, 'void': False}, 'families': {}}
    for part in parts:
        merge_log(out['fallback_log'], part['fallback_log'])
        for name, fam in part['families'].items():
            into = out['families'].setdefault(name, {'files': {}, 'keys': {}})
            for k, n in fam['files'].items():
                into['files'][k] = into['files'].get(k, 0) + n
            for k, c in fam['keys'].items():
                into['keys'][k] = [a + b for a, b in zip(into['keys'].get(k, [0, 0, 0]), c)]
    return out


def keys_report(counts, bar):
    """R2 from its counts: per family, a key in at least five files is
    gated alone, the tag keys in fewer are pooled into one row gated on
    their occurrences, and any other key in fewer is listed but not
    gated."""
    out = {'bar': bar, 'fallback_log': counts['fallback_log'], 'families': {}, 'counts': counts}
    for name in sorted(counts['families']):
        fam = counts['families'][name]
        rows, pooled = [], {'key': TAGS_POOLED, 'keys': 0, 'occurrences': 0, 'agree': 0}
        for k in sorted(fam['keys']):
            seen, agree, covered = fam['keys'][k]
            if seen < 5 and '.tags.' in k:
                pooled['keys'] += 1
                pooled['occurrences'] += seen
                pooled['agree'] += agree
                continue
            rows.append({'key': k, 'files': seen, 'agree': agree, 'via_known_divergence': covered,
                         'gated': seen >= 5, 'rate': round(100.0 * agree / seen, 2)})
        if pooled['keys']:
            pooled.update(gated=True, rate=round(100.0 * pooled['agree'] / pooled['occurrences'], 2))
            rows.append(pooled)
        for row in rows:
            total = row.get('files', row.get('occurrences'))
            row['below'] = row['gated'] and row['agree'] * 100.0 < bar * total
        out['families'][name] = {'files': fam['files'], 'keys': rows,
                                 'below': [r['key'] for r in rows if r['below']]}
    out['unmeasured'] = [name for name, d in sorted(out['families'].items())
                         if not any(n for k, n in d['files'].items() if k != 'ffprobe failed')]
    return out


def print_keys_report(summary, top):
    print(json.dumps({'fallback_log': summary['fallback_log'],
                      'families': {f: {'files': d['files'], 'gated': sum(r['gated'] for r in d['keys']),
                                       'below': len(d['below'])}
                                   for f, d in summary['families'].items()}}))
    for fam, d in summary['families'].items():
        low = sorted((r for r in d['keys'] if r['below']), key=lambda r: (r['rate'], r['key']))
        for r in low[:top]:
            total = r.get('files', r.get('occurrences'))
            print(f"{fam:13} {r['rate']:6.2f}% {r['agree']}/{total} {r['key']}")
    if summary['fallback_log']['void']:
        print('VOID: the fallback log holds a line')
        return 2
    if len(summary['unmeasured']) == len(summary['families']):
        print('UNMEASURED: no file of the set that ffprobe reads')
        return release_round.UNMEASURED
    for name in summary['unmeasured']:
        print(f'UNMEASURED {name}: no file of the family that ffprobe reads, so it misses R2')
    return 1 if summary['unmeasured'] or any(d['below'] for d in summary['families'].values()) else 0


def write_keys_outputs(opts, records, counts):
    summary = keys_report(counts, opts.bar)
    if opts.summary:
        pathlib.Path(opts.summary).write_text(json.dumps(summary, indent=1) + '\n')
    if opts.report:
        examples = collections.defaultdict(dict)
        for r in records:
            for k, e in r.get('examples', {}).items():
                if len(examples[r['family']].setdefault(k, [])) < 5:
                    examples[r['family']][k].append({'file': r['file'], 'path': e[0],
                                                     'ffprobe': e[1], 'coreav': e[2]})
        rejected = [{'file': r['file'], 'family': r['family'], 'rejected': r['rejected']}
                    for r in records if r.get('rejected')]
        pathlib.Path(opts.report).write_text(json.dumps(
            {**summary, 'rejected': rejected, 'examples': examples}, indent=1) + '\n')
    return print_keys_report(summary, opts.top)


def cmd_keys_010(opts):
    files = media_files(opts.media)
    if opts.list:
        files = [pathlib.Path(line.strip()) for line in open(opts.list) if line.strip()]
    files = [p for p in files if release_round.family(p)]
    patterns = release_round.key_patterns()
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        records = list(pool.map(lambda p: key_record(p, compare_all(opts.coreav, p), patterns), files))
    records.append({'fallback_log': release_round.fallback_verdict(ENV['COREAV_FALLBACK_LOG'])})
    if opts.records:
        pathlib.Path(opts.records).write_text(''.join(json.dumps(r) + '\n' for r in records))
    return write_keys_outputs(opts, records, keys_counts(records))


def cmd_keys_report(opts):
    records = [json.loads(line) for path in opts.records for line in open(path) if line.strip()]
    parts = [keys_counts(records)] + [json.loads(pathlib.Path(p).read_text())['counts'] for p in opts.summaries]
    return write_keys_outputs(opts, records, merge_counts(parts))


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='mode', required=True)
    for name in ('commands', 'keys'):
        p = sub.add_parser(name)
        p.add_argument('--coreav', required=True)
        p.add_argument('--set', choices=('v1', '0.1.0'), default='v1',
                       help='commands-v1 (B1 to B3a), or commands-0.1.0 (R1, R2)')
        p.add_argument('--media', nargs='+', default=[])
        p.add_argument('--report')
        p.add_argument('--top', type=int, default=40)
        p.add_argument('--jobs', type=int, default=os.cpu_count() or 4)
        if name == 'commands':
            p.add_argument('--baseline')
            p.add_argument('--write-baseline')
        if name == 'keys':
            p.add_argument('--list')
            p.add_argument('--bar', type=float, default=98.0)
            p.add_argument('--records', help='with --set 0.1.0: one line per file (private)')
            p.add_argument('--summary', help='with --set 0.1.0: the counts, with no file named')
    p = sub.add_parser('keys-report')
    p.add_argument('--records', nargs='+', default=[], help='per-file records of `keys --set 0.1.0`')
    p.add_argument('--summaries', nargs='+', default=[], help='--summary files of other runs, added up')
    p.add_argument('--report')
    p.add_argument('--summary')
    p.add_argument('--top', type=int, default=40)
    p.add_argument('--bar', type=float, default=98.0)
    opts = parser.parse_args()
    if opts.mode == 'keys-report':
        return cmd_keys_report(opts)
    if opts.set == 'v1':
        if opts.mode == 'keys' and (opts.records or opts.summary):
            parser.error('--records and --summary go with --set 0.1.0')
        return cmd_commands(opts) if opts.mode == 'commands' else cmd_keys(opts)
    with tempfile.TemporaryDirectory(prefix='coreav-fallback-') as tmp:
        ENV.update(release_round.environment(pathlib.Path(tmp) / 'fallback.log'))
        return cmd_commands_010(opts) if opts.mode == 'commands' else cmd_keys_010(opts)


if __name__ == '__main__':
    sys.exit(main())
