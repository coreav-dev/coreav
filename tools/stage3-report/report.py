"""Renders the stage 3 correctness report (docs/design/stage3-report.md)
from the step results of run.py: returns the JSON twin and the Markdown.
Rows of the engines run.py's `extra_hostile` module adds come from that
module, when this tree has it."""
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
try:
    import extra_hostile
except ImportError:
    extra_hostile = None

# Every known divergence of the probe overlays (docs/known-divergences.json,
# docs/known-divergences-mkv.json, docs/known-divergences-ts.json) dated
# against the bars of their container: the stage 3 bars, and for transport
# streams the bars of docs/design/mpegts.md (fixed in 1930140, before its
# entries were written)
# (docs/design/stage3-report.md, "Known divergences and the bars"). The bars
# were set before any entry was written, so no entry is
# from before them: PRE_BAR is empty, and the report gives every number
# with all entries and with the pre-bar entries only, side by side.
# Each entry: the merged pull request and squash commit that added it; those
# that later changed its predicate or coverage; and the fingerprint of its
# JSON record and predicate source that the last of them left, which
# tools/oracle/tests/test_kd.py recomputes, so a change to either is dated
# here in the pull request that makes it.
PRE_BAR = ()
ADDED_AFTER_THE_BARS = {
    'KD-STRICT': ('#12 dd853a6', ('#116 88a660b',), '3cc36879938e'),
    'KD-STTS-SHORT': ('#12 dd853a6', ('#105 2199176', '#109 0bca9b9', '#160 d7d79c2', '#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'e3bab2becda4'),
    'KD-EDIT-DISCARD': ('#12 dd853a6', ('#118 35f1801', '#125 a213539', '#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744', '#283 7d10512'), '00ca89874db3'),
    'KD-CTTS': ('#12 dd853a6', ('#118 35f1801', '#142 36b1878', '#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), '6681e0087ac7'),
    'KD-PCM': ('#12 dd853a6', ('#118 35f1801', '#125 a213539', '#126 c6236a6', '#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'b26be50da76e'),
    'KD-AAC-IMPLICIT-SBR': ('#16 1bce9e8', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744', '#283 7d10512'), 'ff0f07706190'),
    'KD-AAC-LC-UNNAMED': ('#16 1bce9e8', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744', '#283 7d10512'), '757f9b8cb849'),
    'KD-DECLARED-SIZE': ('#70 060ed50', (), '998a82b33727'),
    'KD-QT-TEXT': ('#69 364d038', ('#104 3c771c2', '#136 6aaafb2', '#148 61a60b8'), 'f5ed8c040971'),
    'KD-EDIT-SNAP': ('#107 2d79137', ('#142 36b1878', '#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), '4e3bedc4881c'),
    'KD-EDIT-DURATION': ('#109 0bca9b9', ('#153 31b3951', '#142 36b1878', '#160 d7d79c2', '#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'c8d41e6376f5'),
    'KD-FRAG-START': ('#142 36b1878', ('#160 d7d79c2', '#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'dab4e31f9571'),
    'KD-NOT-ALLOWED': ('#104 3c771c2', ('#136 6aaafb2', '#148 61a60b8'), '76cfde005b13'),
    'KD-COVER-ART': ('#136 6aaafb2', ('#148 61a60b8',), 'ff33419ddff0'),
    'KD-CHAPTER-TEXT': ('#148 61a60b8', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'ce5fcdd8cf77'),
    'KD-EDIT-MULTI': ('#153 31b3951', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'b6af7fc381cb'),
    'KD-TRUN-SHIFT': ('#160 d7d79c2', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), '37c58ff8d579'),
    'KD-CTTS-UNSIGNED': ('#160 d7d79c2', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), '2f637a1ceb60'),
    'KD-PCM-END': ('#160 d7d79c2', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'ce6556508853'),
    'KD-EDIT-UNPLACED': ('#160 d7d79c2', ('#162 5316840', '#161 1666715', '#187 f9fa510', '#254 8470744'), 'f10d5ea0fd6b'),
    'KD-TEXT-END': ('#161 1666715', ('#187 f9fa510', '#254 8470744'), 'b312da9faf02'),
    'KD-MKV-DTS': ('#51 13600fb', (), 'ca5bbdc45a5d'),
    'KD-MKV-LACE-UNTIMED': ('#51 13600fb', (), 'a4b31d54f98f'),
    'KD-MKV-LACE-SPLIT': ('#51 13600fb', (), '3452a757df1d'),
    'KD-MKV-AUDIO-KEYFRAME': ('#51 13600fb', (), 'c6f638244bea'),
    'KD-MKV-VFW-TIMESTAMPS': ('#51 13600fb', (), '9f0e067225d2'),
    'KD-MKV-WEBM-WEBVTT': ('#51 13600fb', (), '1a8d72d9e3c8'),
    'KD-MKV-ENCRYPTED': ('#51 13600fb', (), '9a5455902d33'),
    'KD-MKV-STREAM-DURATION': ('#51 13600fb', (), 'c3f8ba82ff6e'),
    'KD-MKV-FMT-ROUNDING': ('#51 13600fb', (), 'b0efb6716132'),
    'KD-MKV-UNREAD-BITSTREAM': ('#51 13600fb', (), '35665ca38010'),
    'KD-MKV-HEVC-RANGE': ('#51 13600fb', (), 'a775c14aa3bb'),
    'KD-MKV-ATTACHMENT': ('#51 13600fb', (), '54934863283f'),
    'KD-MKV-DECLARED-SIZE': ('#90 5b3148e', (), 'a635171c5ba2'),
    'KD-MKV-TAG-LANGUAGE': ('#255 bc1adbd', (), 'bd755749a2c6'),
    'KD-TS-END-ESTIMATE': ('#247 cdcd6e4', ('#284 f7f135e', '#307 e4f177a'), '452d4fb988a3'),
    'KD-TS-DUPLICATE': ('#247 cdcd6e4', ('#284 f7f135e', '#307 e4f177a'), '19529b41a401'),
    'KD-TS-FRAME-TIME': ('#284 f7f135e', ('#307 e4f177a',), 'a7fd80dbbbca'),
    'KD-TS-STREAM-TIMING': ('#284 f7f135e', ('#307 e4f177a',), '1db561204950'),
    'KD-TS-AAC-PROFILE': ('#284 f7f135e', ('#307 e4f177a',), '0a767a96a39c'),
    'KD-TS-STREAM-ORDER': ('#284 f7f135e', ('#307 e4f177a',), '300db2247a8f'),
    'KD-TS-TELETEXT-SIZE': ('#284 f7f135e', ('#307 e4f177a',), '35b0c12741d3'),
    'KD-TS-DECODE-TIME': ('#307 e4f177a', (), '75c3c491dffa'),
    'KD-TS-POSITION': ('#284 f7f135e', ('#307 e4f177a',), '4950c91d1682'),
}
# Entries no longer in the overlays: the pull request and commit that added
# them, and the one that retired them.
RETIRED = {'KD-FMT-ORIGIN': ('#12 dd853a6', '#105 2199176')}
FAMILIES = {'isobmff': 'MP4/MOV/M4A/3GP', 'matroska': 'Matroska/WebM',
            'mpegts': 'MPEG transport streams'}
FILE_BAR, PACKET_BAR = 98, 99


def pct(n, d):
    return f'{100 * n / d:.2f}%' if d else 'n/a'


def agreeing(level):
    return level['direct'] + level['via_kd']


def without_added(level, kind, files=None):
    """The level counted with the pre-bar entries only: a file (streams)
    covered by any other entry is uncovered, and so is every packet another
    entry covers. An entry missing from both lists counts as added after
    the bars."""
    if kind == 'packets':
        moved = sum(n for k, n in level['by_kd'].items() if k not in PRE_BAR)
        return {**level, 'via_kd': level['via_kd'] - moved, 'uncovered': level['uncovered'] + moved}
    if files is None and level.get('via_kd'):
        # Without the entries each covered file used, the pre-bar count
        # cannot be told from the full one.
        raise ValueError('a corpus summary without covered_kd_sets: rerun the corpus step')
    moved = sum(1 for f in (files or []) if not f or set(f) - set(PRE_BAR))
    return {**level, 'via_kd': level['via_kd'] - moved, 'uncovered': level['uncovered'] + moved}


# Per-file lists stay in the work directory: the committed report holds
# aggregate counts only.
PER_FILE = ('files_with_uncovered', 'covered_kd_sets', 'examples', 'uncovered_items', 'covered_items')


def public(value):
    if isinstance(value, dict):
        return {k: public(v) for k, v in value.items() if k not in PER_FILE}
    if isinstance(value, list):
        return [public(v) for v in value]
    return value


def status(ok):
    return '**Met**' if ok else '**Not met**'


def corpus_bars(summary):
    """The bar numbers of one family over all its real files. Files that
    could not be checked (a failed download, a comparison the kernel
    stopped) count against the files bar; files `ffprobe` cannot read are
    already outside the overlay's items. Also the numbers
    with the pre-bar entries only (`PRE_BAR`)."""
    a = summary['all']
    st, pk = a['levels']['streams'], a['levels']['packets']
    # No record at all (a stopped run) counts like a file not checked.
    unchecked = sum(a['errors'].values()) + a.get('missing', 0)
    old_st = without_added(st, 'streams', a.get('covered_kd_sets'))
    old_pk = without_added(pk, 'packets')
    # The packets bar is in the reference's packet unit; the per-sample
    # count is reported beside it and gates nothing (docs/design/
    # stage3-report.md, "Packet unit").
    ps = pk.get('per_sample', pk)
    return {'files': [agreeing(st), st['items'] + unchecked], 'files_checked_only': [agreeing(st), st['items']],
            'packets': [agreeing(pk), pk['items']], 'packets_per_sample': [agreeing(ps), ps['items']],
            'unchecked_files': unchecked,
            'files_pre_bar_only': [agreeing(old_st), old_st['items'] + unchecked],
            'packets_pre_bar_only': [agreeing(old_pk), old_pk['items']],
            'files_met': agreeing(st) * 100 >= FILE_BAR * (st['items'] + unchecked),
            'packets_met': agreeing(pk) * 100 >= PACKET_BAR * pk['items'],
            'mixed_tools': a.get('mixed_tools', False)}


def fixtures_section(f):
    lines = ['## Fixtures', '',
             f"Generated with the pinned ffmpeg (`scripts/gen-fixtures.sh`: {f['generated']['isobmff']} files; "
             f"`scripts/gen-fixtures-mkv.sh`: {f['generated']['matroska']} files), never committed. Bar: 100% of "
             "each check. Packets are counted in `ffprobe`'s packet unit; the count with "
             'one packet per constant-size PCM sample is shown below it (docs/design/stage3-report.md, '
             '"Packet unit").', '',
             '| Check | Items | Direct | Via known divergence | Uncovered | Status |', '|---|---:|---:|---:|---:|---|']
    rows = []
    for family, probe in f['probe'].items():
        for level in ('streams', 'packets'):
            lv = probe['levels'][level]
            ok = lv['uncovered'] == 0 and lv['items'] > 0
            rows.append((f"probe, {FAMILIES[family]}, {level} oracle", lv, ok))
            lines.append(f"| probe, {FAMILIES[family]}, {level} oracle | {lv['items']:,} | {lv['direct']:,} | "
                         f"{lv['via_kd']:,} ({', '.join(f'{k} {v:,}' for k, v in sorted(lv['by_kd'].items())) or '–'}) "
                         f"| {lv['uncovered']:,} | {status(ok)} |")
            ps = lv.get('per_sample')
            if ps and ps['items'] != lv['items']:
                lines.append(f"| probe, {FAMILIES[family]}, packets per PCM sample (reported, not a bar) | "
                             f"{ps['items']:,} | {ps['direct']:,} | {ps['via_kd']:,} "
                             f"({', '.join(f'{k} {v:,}' for k, v in sorted(ps['by_kd'].items())) or '–'}) "
                             f"| {ps['uncovered']:,} | – |")
    lines += ['', '| Fixture check (as CI runs it) | Result | Status |', '|---|---|---|']
    for c in f['checks']:
        count = f"{c['count'][0]}/{c['count'][1]}" if c.get('count') else ''
        lines.append(f"| {c['name']} | {count or c.get('summary', '')[:80]} | {status(c['passed'])} |")
    return lines


def corpus_table(summary, family):
    lines = ['| Split | Files | Checked | ffprobe fails (excluded) | Files agreeing (direct + via known '
             "divergence) | Packets agreeing, in `ffprobe`'s packet unit (direct + via known divergence) |",
             '|---|---:|---:|---:|---|---|']
    for split in ('development', 'holdout', 'all'):
        s = summary[split]
        st, pk = s['levels'].get('streams'), s['levels'].get('packets')
        if not st:
            continue
        lines.append(f"| {split} | {s['files']} | {s['checked']} | {s['ffprobe_failed']} | "
                     f"{agreeing(st):,}/{st['items']:,} ({pct(agreeing(st), st['items'])}; "
                     f"{st['direct']:,} + {st['via_kd']:,}) | {agreeing(pk):,}/{pk['items']:,} "
                     f"({pct(agreeing(pk), pk['items'])}; {pk['direct']:,} + {pk['via_kd']:,}) |")
    return lines


COMPAT_BAR = 98


def compat_bars(summary):
    """The compat bar over all of a family's real files: per key, the files whose value agrees (directly
    or through a known divergence checked on that file) and directly alone. The bar is every key on at least
    COMPAT_BAR% of the files that print it, where a file whose compat check
    failed or did not run counts against every key (it could have printed
    any). None without compat records."""
    c = summary['all'].get('compat')
    if not c or not (c['records'] + c.get('not_run', 0)):
        return None
    unchecked = c['errors'] + c.get('not_run', 0)
    keys = {k: {'agree': v['direct'] + v['kd'], 'direct': v['direct'], 'files': v['files'] + unchecked}
            for k, v in c['keys'].items()}
    worst = min(keys.items(), key=lambda kv: (kv[1]['agree'] / kv[1]['files'], kv[0])) if keys else None
    worst_direct = min(keys.items(), key=lambda kv: (kv[1]['direct'] / kv[1]['files'], kv[0])) if keys else None
    met = bool(keys) and all(v['agree'] * 100 >= COMPAT_BAR * v['files'] for v in keys.values())
    return {'keys': keys, 'worst': worst, 'worst_direct': worst_direct, 'met': met, 'compared': c['compared'],
            'rejected_unexplained': c['rejected_unexplained'], 'errors': c['errors'],
            'not_run': c.get('not_run', 0), 'records': c['records']}


def compat_lines(b):
    lines = ['', f"`--ffprobe-compat`: every key it prints equal to `ffprobe`'s on at least "
             f"{COMPAT_BAR}% of the files that print it, a known divergence counting only where the overlay "
             f"checked it on that field, on that file and stream, and the printed value is the one it checked "
             f"(`scripts/compat_check.py`); a file whose compat check failed or did not "
             f"run counts against every key. Files compared: {b['compared']:,}; check failed: {b['errors']:,}; "
             f"records without the check: {b['not_run']:,}. Files `ffprobe` reads that CoreAV rejects with no "
             f"known divergence to excuse it (counted beside the bar; the streams bar above already counts them): "
             f"{b['rejected_unexplained']:,}. **{'Met' if b['met'] else 'Not met'}**"
             + (f" (lowest key `{b['worst'][0]}`)." if b['worst'] else '.'), '',
             '| Key | Files | Agreeing (direct + via known divergence) | Direct only | Status |',
             '|---|---:|---|---|---|']
    for key, v in sorted(b['keys'].items()):
        ok = v['agree'] * 100 >= COMPAT_BAR * v['files']
        lines.append(f"| `{key}` | {v['files']:,} | {v['agree']:,} ({pct(v['agree'], v['files'])}) | "
                     f"{v['direct']:,} ({pct(v['direct'], v['files'])}) | {status(ok)} |")
    return lines


def corpus_section(c):
    lines = ['## Real-file corpus: probe', '',
             'Every `set: real` file of corpus v1, downloaded from R2 with its SHA-256 checked, '
             'captured by `ffprobe` and the adapter at the streams and packets levels, compared and overlaid with '
             'the known divergences (`tools/stage3-report/corpus.py`). A file agrees when every gating field of '
             'its streams (the streams level) agrees directly or through a known divergence; its packets count '
             'in the packets bar; files `ffprobe` cannot read are left out of the '
             f'denominator. Bars: at least {FILE_BAR}% of files and {PACKET_BAR}% of packets per '
             "family. Packets are counted in `ffprobe`'s packet unit: a constant-size PCM stream counts one unit "
             'per `ffprobe` packet, which agrees only when CoreAV\'s samples tile it exactly. The count with one '
             'packet per PCM sample, the unit before this method, is reported beside it and is not a bar '
             '(docs/design/stage3-report.md, "Packet unit").', '']
    data = {}
    for family, summary in c['families'].items():
        st, pk = summary['all']['levels']['streams'], summary['all']['levels']['packets']
        bars = data[family] = corpus_bars(summary)
        lines += [f"### {FAMILIES[family]}", '']
        for r in summary.get('runs', []):
            lines.append(f"- run {r['time_utc']}: commit `{r['commit'][:12]}`{' (uncommitted changes)' if r['dirty'] else ''}, "
                         f"{r['files']} file{'' if r['files'] == 1 else 's'}"
                     f"{', recheck ' + r['recheck'] if r.get('recheck') else ''}")
        for t in summary['all'].get('tools', []):
            if t:
                harness = f", harness `{t['harness']}`" if t.get('harness') else ''
                lines.append(f"- records made by adapter `{t['adapter']}`, comparator `{t['oracle']}`, overlay "
                             f"`{t['overlay']}`{harness} (SHA-256 prefixes)"
                             + ''.join(f"; {t[k]}" for k in ('ffprobe', 'mediainfo') if t.get(k)))
        if bars['mixed_tools']:
            lines.append('- **The records were made by different tool versions**: recheck them all before '
                         'reading these numbers.')
        lines += [''] + corpus_table(summary, family) + ['']
        f, p = bars['files'], bars['packets']
        lines.append(f"Files: {status(bars['files_met'])} ({f[0]:,}/{f[1]:,}, {pct(*f)} against {FILE_BAR}%, "
                     + (f"the {bars['unchecked_files']} files not checked counted as disagreeing). "
                        if bars['unchecked_files'] else 'every file checked). ') +
                     f"Packets: {status(bars['packets_met'])} ({pct(*p)} against {PACKET_BAR}%, over the files "
                     "checked, in `ffprobe`'s packet unit).")
        ps = pk.get('per_sample', pk)
        if ps['items'] != pk['items']:
            sp = bars['packets_per_sample']
            lines.append(f"Per-sample count (one packet per constant-size PCM sample; reported, not a bar): "
                         f"{sp[0]:,}/{sp[1]:,} ({pct(*sp)}; {ps['direct']:,} direct + {ps['via_kd']:,} via known "
                         f"divergence, KD-PCM {ps['by_kd'].get('KD-PCM', 0):,}).")
        of, op = bars['files_pre_bar_only'], bars['packets_pre_bar_only']
        pre_bar = ', '.join(PRE_BAR) or 'none: every entry was added or changed after them'
        lines += ['', 'Agreement with every known divergence, and with only the entries that existed when the '
                  f'bars were fixed ({pre_bar}; `PRE_BAR` and `ADDED_AFTER_THE_BARS` in '
                  '`tools/stage3-report/report.py`):', '',
                  '| | With every entry | With pre-bar entries only |', '|---|---|---|',
                  f"| Files (bar {FILE_BAR}%) | {f[0]:,}/{f[1]:,} ({pct(*f)}) | {of[0]:,}/{of[1]:,} ({pct(*of)}) |",
                  f"| Packets (bar {PACKET_BAR}%) | {p[0]:,}/{p[1]:,} ({pct(*p)}) | {op[0]:,}/{op[1]:,} "
                  f"({pct(*op)}) |"]
        cb = compat_bars(summary)
        if cb:
            lines += compat_lines(cb)
        lines += ['', "Known divergences used (files at the streams level; packets in `ffprobe`'s unit):", '']
        for k in sorted(set(st['by_kd']) | set(pk['by_kd'])):
            lines.append(f"- {k}: {st['by_kd'].get(k, 0):,} files; {pk['by_kd'].get(k, 0):,} packets")
        a = summary['all']
        if a['errors']:
            lines += ['', f"Not checked: {', '.join(f'{v} {k}' for k, v in a['errors'].items())}."]
        lines += ['', 'Uncovered disagreements by cause (`tools/stage3-report/causes.py`; a file counts under '
                  'every cause it shows, its packets under the first differing packet field):', '',
                  '| Cause | Files | Packets (`ffprobe`\'s unit) | Codecs (streams or packets) | Containers |',
                  '|---|---:|---:|---|---|']
        for name, c in (summary.get('causes') or {}).items():
            codecs = ', '.join(f'{k} {v:,}' for k, v in c['codecs'].items()) or '–'
            containers = ', '.join(f'{k} {v}' for k, v in c['containers'].items())
            lines.append(f"| {name} | {c['files']:,} | {c['packets']:,} | {codecs} | {containers} |")
        lines.append('')
    return lines, data


# The classes of tools/mediainfo-oracle (its triage of CoreAV and ffprobe
# disagreements), with what MediaInfo read.
TRIAGE = (('ffprobe-alone', 'agrees with CoreAV'), ('coreav-alone', 'agrees with ffprobe'),
          ('mediainfo-between', 'within tolerance of both'), ('all-differ', 'differs from both'),
          ('not-comparable', 'has no value'))


def mediainfo_lines(title, s):
    totals = {}
    for counter in s['triage'].values():
        for k, v in counter.items():
            totals[k] = totals.get(k, 0) + v
    reading = ', '.join(f"{label} {totals.get(key, 0):,}" for key, label in TRIAGE)
    probe = '; '.join(f'{k}: {v}' for k, v in s['probe_outcomes'].items())
    return [f"- **{title}** ({s['files']} files). Probe outcomes: {probe}. CoreAV–ffprobe disagreements on "
            f"gating fields: {sum(totals.values()):,}; MediaInfo {reading}."]


def hostile_section(h):
    n = h['native']
    native_ok = n.get('passed') and not n.get('failures')
    reject_ok = all(r['allowed_streams'] == 0 for r in h['reject_cases_probed_ok'])
    # The extra engines' row, when run.py ran them and this tree renders them.
    extra = extra_hostile.hostile_row(h) if extra_hostile and h.get('wasm') is not None else None
    lines = ['## Hostile-input suite', '',
             f"{h['cases']} files from `tools/hostile/generate.py` (manifest SHA-256 "
             f"`{h['manifest_sha256'][:16]}…`), by part: " +
             '; '.join(f"{p} {v['reject']} reject, {v['bounded']} bounded" for p, v in h['by_part'].items()) + '.',
             '', '| Build | Result | Status |', '|---|---|---|',
             f"| native `coreav` (both output modes, 10 s and 1 GiB per case) | {h['cases']} cases, "
             f"{len(n.get('failures', []))} failures | {status(native_ok)} |"]
    if extra:
        lines.append(extra[0])
    lines += [f"| `reject` cases whose probe succeeds | {len(h['reject_cases_probed_ok'])}, each with every stream "
              f"dropped (no allowed stream) | {status(reject_ok)} |",
              '']
    if extra:
        lines += extra[2]
    lines += ['### CVE route files', '',
             'Each file routes default FFmpeg to the component the public description names, by content, '
             'whatever the file is called; it carries no crafted overflow. CoreAV must reject it at probe time. '
             '(`docs/design/stage3-report.md`.)', '',
             '| File | CVE | Component (NVD description) |', '|---|---|---|']
    for c in h['cves']:
        lines.append(f"| `{c['name']}` | {c['cve']} | {c.get('component') or 'MagicYUV decoder, out-of-bounds write'} |")
    # The JSON twin holds the bars of this tree's own engines only; the extra
    # engines' rows are in the Markdown.
    return lines, {'native_met': native_ok, 'reject_cases_met': reject_ok}


def summary_section(fixtures, corpus, hostile):
    """One row per bar."""
    rows = []
    if fixtures:
        probe = all(p['levels'][lv]['uncovered'] == 0 for p in fixtures['probe'].values() for lv in p['levels'])
        checks = all(c['passed'] for c in fixtures['checks'])
        missed = [f"{FAMILIES[family]} {level} {lv['uncovered']:,}" for family, p in fixtures['probe'].items()
                  for level, lv in p['levels'].items() if lv['uncovered']]
        rows.append(('Fixtures: probe stream and packet oracles, MP4/MOV and Matroska', '100%',
                     'uncovered items: ' + ', '.join(missed) if missed
                     else 'every item agrees, directly or through a known divergence', probe))
        rows.append(('Fixtures: --ffprobe-compat and every stream-copy primitive', '100%',
                     f"{sum(c['passed'] for c in fixtures['checks'])}/{len(fixtures['checks'])} checks pass", checks))
    for family, s in ((corpus or {}).get('families') or {}).items():
        b = corpus_bars(s)
        without = '; with pre-bar entries only: '
        rows.append((f'Corpus, {FAMILIES[family]}: files', f'≥ {FILE_BAR}%',
                     f"{b['files'][0]:,}/{b['files'][1]:,} ({pct(*b['files'])}){without}"
                     f"{b['files_pre_bar_only'][0]:,}/{b['files_pre_bar_only'][1]:,} "
                     f"({pct(*b['files_pre_bar_only'])})", b['files_met']))
        per_sample = (f"; per PCM sample (not a bar): {pct(*b['packets_per_sample'])}"
                      if b['packets_per_sample'] != b['packets'] else '')
        rows.append((f"Corpus, {FAMILIES[family]}: packets (`ffprobe`'s packet unit)", f'≥ {PACKET_BAR}%',
                     f"{b['packets'][0]:,}/{b['packets'][1]:,} ({pct(*b['packets'])}){without}"
                     f"{b['packets_pre_bar_only'][0]:,}/{b['packets_pre_bar_only'][1]:,} "
                     f"({pct(*b['packets_pre_bar_only'])})" + per_sample,
                     b['packets_met']))
        cb = compat_bars(s)
        if cb:
            if cb['worst']:
                (wk, w), (dk, d) = cb['worst'], cb['worst_direct']
                result = (f"{len(cb['keys'])} keys; lowest `{wk}` {w['agree']:,}/{w['files']:,} "
                          f"({pct(w['agree'], w['files'])}); directly: lowest `{dk}` {d['direct']:,}/{d['files']:,} "
                          f"({pct(d['direct'], d['files'])})")
            else:
                result = (f"no key compared; check failed on {cb['errors']:,} files, not run on "
                          f"{cb['not_run']:,}")
            rows.append((f'Corpus, {FAMILIES[family]}: `--ffprobe-compat`, every key', f'≥ {COMPAT_BAR}% of files',
                         result, cb['met']))
    if hostile:
        rows.append(('Hostile suite, native: rejected or bounded, no crash', 'every case',
                     f"{hostile['cases']} cases, {len(hostile['native'].get('failures', []))} failures",
                     hostile['native'].get('passed') and not hostile['native'].get('failures')))
        if extra_hostile and hostile.get('wasm') is not None:
            rows.append(extra_hostile.summary_row(hostile))
    lines = ['## Summary', '', '| Criterion | Bar | Result | Status |', '|---|---|---|---|']
    lines += [f'| {name} | {bar} | {result} | {status(ok)} |' for name, bar, result, ok in rows]
    return lines + ['', 'Second oracle (MediaInfo): informational, below.', '']


def render(results):
    build, fixtures, corpus, hostile = (results.get(k) for k in ('build', 'fixtures', 'corpus', 'hostile'))
    data = {'schema': 'coreav.stage3-report/1', 'steps': {k: (v or {}).get('provenance') for k, v in results.items()}}
    lines = ['# Stage 3 correctness report', '',
             'Regenerated by `python3 -I tools/stage3-report/run.py --work DIR` (`docs/design/stage3-report.md`); '
             'that note states the bars. Each section names the commit it ran on.', '']
    commits = {k: v['provenance'] for k, v in results.items() if v}
    for k, p in commits.items():
        lines.append(f"- {k}: commit `{p['commit'][:12]}`{' (with uncommitted changes)' if p['dirty'] else ''}, "
                     f"{p['time_utc']}")
    if build:
        t = build['tools']
        tools = f"Tools: {t['ffprobe']}; {t['mediainfo']}. "
        if extra_hostile:
            tools += extra_hostile.tools_note(build, hostile)
        lines += ['', tools.rstrip()]
    lines.append('')
    lines += summary_section(fixtures, corpus, hostile)
    if fixtures:
        lines += fixtures_section(fixtures) + ['']
        data['fixtures'] = fixtures
    if corpus:
        section, bars = corpus_section(corpus)
        data['corpus'] = {'bars': bars, 'families': public(corpus['families']),
                          'mediainfo': corpus.get('mediainfo'),
                          'known_divergences': {
                              'pre_bar': list(PRE_BAR),
                              'added_after_the_bars': {k: {'added': v[0], 'changed': list(v[1])}
                                                       for k, v in ADDED_AFTER_THE_BARS.items()},
                              'retired': {k: {'added': v[0], 'retired': v[1]} for k, v in RETIRED.items()}}}
        lines += section
    lines += ['## Second oracle: MediaInfo', '',
              'The three-way stream report: each CoreAV–ffprobe disagreement on a gating field, '
              'classified by what MediaInfo reads. Informational, no bar; corpus counts are aggregate '
              'only.', '']
    for name, src in (('fixtures', fixtures), ('corpus', corpus)):
        for family, s in ((src or {}).get('mediainfo') or {}).items():
            lines += mediainfo_lines(f'{FAMILIES[family]}, {name}', s)
    lines.append('')
    if hostile:
        section, data['hostile'] = hostile_section(hostile)
        lines += section + ['']
        data['hostile_detail'] = {k: v for k, v in hostile.items()
                                  if not (extra_hostile and k in extra_hostile.KEYS)}
    lines += ['## The demo', '']
    demo = extra_hostile.demo_lines(hostile) if extra_hostile else None
    lines += demo or ['`tools/stage3-report/demo.sh [FILE]` runs FFmpeg 7.0.2 on the file in a sandbox (no network, '
                      'every mount read-only, resource limits) and shows it decode the file, then `coreav probe '
                      '--strict` and `coreav check` reject it.', '']
    lines += [
              '## Not in this report', '',
              '- The 24 CPU-hour fuzz campaign of the release criteria.',
              '- Speed: each primitive has its own measurement.', '']
    return data, '\n'.join(lines)


if __name__ == '__main__':
    import sys
    print(json.dumps(render(json.load(open(sys.argv[1])))[0], indent=1))
