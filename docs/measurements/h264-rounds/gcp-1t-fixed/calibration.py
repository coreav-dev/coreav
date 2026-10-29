"""The calibration of the one-thread class at a fixed frequency (docs/design/h264.md, "The one-thread class at a fixed
frequency"): its bars H-1 to V-2, from the two runs of scripts/h264_parity_gcp.py.

    python3 -I calibration.py ES_JSON AA_DIR NA_DIR [--json OUT]

ES_JSON is the training set's es.json; AA_DIR and NA_DIR are the --out directories of the two runs. A run whose reports
do not hold every stream of ES_JSON exactly once, each with every command timed in every round of its call (the per-round
times in `rounds`, whose medians must be the row's `seconds` and `null`), is refused (exit 2) with the streams named: no
bar is judged on part of the set, or on a summary its rounds do not support. Exit 0 when every bar of the class's acceptance is met, 1 when one is not.
"""
import argparse
import glob
import json
import math
import os
import statistics
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))))
sys.path.insert(0, os.path.join(ROOT, 'scripts'))
import h264_parity as harness  # noqa: E402

CLASSES = ('sd', '720p', '1080p', '4k')
MODES = ('discard', 'write')
CPU = ('8581C', '6', '207')
ACCEPT = ('H-1', 'H-2', 'A-1', 'A-2', 'A-3', 'A-4', 'N-1', 'N-2')


def load(d, es, arms, null_arm):
    """The rows of a run's four reports, checked against the set: every stream once, every command with a time."""
    rows, reports, rounds = [], [], {}
    for cls in CLASSES:
        with open(os.path.join(d, f'g.{cls}.json')) as f:
            r = json.load(f)
        reports.append(r)
        rows += r['streams']
        rounds.update((s['stream'], r['rounds']) for s in r['streams'])
    names = [r['stream'] for r in rows]
    missing = sorted(set(es) - set(names))
    extra = sorted({n for n in names if names.count(n) > 1 or n not in es})
    want = [f'{a}_{m}' for a in arms for m in MODES] + ['ff_null', 'ff_raw']
    untimed = sorted(r['stream'] for r in rows if not timed(r, want, null_arm, rounds[r['stream']]))
    if missing or extra or untimed:
        print(f'{d}: refused, the reports do not hold the set: missing {missing}, duplicated or unknown {extra}, '
              f'without every command in every round, or with summaries its rounds do not give {untimed}', file=sys.stderr)
        sys.exit(2)
    return rows, reports


def timed(row, want, null_arm, n):
    """Every command of the row timed in all n rounds, the medians of those times being the row's seconds and null."""
    t = row.get('rounds', {})
    runs = want + [f'{null_arm}~null_{m}' for m in MODES]
    if not isinstance(n, int) or n < 1 or len(row.get('order', [])) != n or not all(
            isinstance(t.get(c), list) and len(t[c]) == n
            and all(isinstance(x, (int, float)) and math.isfinite(x) and x > 0 for x in t[c]) for c in runs):
        return False
    same = lambda a, b: isinstance(b, (int, float)) and math.isclose(a, b, rel_tol=1e-9, abs_tol=1e-12)
    return (all(same(statistics.median(t[c]), row['seconds'].get(c)) for c in want)
            and all(same(statistics.median(t[f'{null_arm}~null_{m}']) / statistics.median(t[f'{null_arm}_{m}']) - 1,
                         row['null'].get(f'{null_arm}_{m}')) for m in MODES))


def med(values):
    lo, hi = harness.interval(values, seed=harness.SEED)
    return {'median': statistics.median(values), 'low': lo, 'high': hi, 'half_width': (hi - lo) / 2, 'n': len(values)}


def by_class(v):
    return {c: statistics.median(x for s, x in v.items() if s.split('.')[2] == c) for c in CLASSES}


def ratio_stats(rows, num, den):
    """Per mode: the median of num/den over the streams with its interval, per class, and the largest deviation."""
    out = {}
    for m in MODES:
        v = {r['stream']: r['seconds'][f'{num}_{m}'] / r['seconds'][f'{den}_{m}'] for r in rows}
        out[m] = {'all': med(list(v.values())), 'class': by_class(v), 'max_dev': max(abs(x - 1) for x in v.values())}
    return out


def host(d, reports):
    """H-1 and H-2 of one run, and its frequency samples, gate checks and references."""
    with open(os.path.join(d, 'verdict.json')) as f:
        v = json.load(f)
    cpu = all(CPU[0] in r['machine']['cpu_model'] and r['machine']['cpu_family'] == CPU[1]
              and r['machine']['cpu_model_number'] == CPU[2] for r in reports)
    with open(os.path.join(d, 'gate-checks.log')) as f:
        checks = [l.split(' ')[:4] for l in f.read().splitlines() if l]
    first_after = all(any(c[0] == 'PASS' and c[1:4] == ['after', cls, '#1'] for c in checks) for cls in CLASSES)
    voids = sorted(os.path.basename(p) for p in glob.glob(os.path.join(d, 'g.*.void*')))
    drift = sum(1 for r in reports for g in r['method']['gates'] if g['kind'] == 'confirm' and g['drifted'])
    with open(os.path.join(d, 'freq.json')) as f:
        freq = json.load(f)
    with open(os.path.join(d, 'gate-refs.json')) as f:
        refs = json.load(f)
    ref = {n: statistics.median(r[n] for r in refs) for n in refs[0] if n not in ('label', 'time')}
    return {'H-1': not v['void'] and cpu, 'H-2': first_after and not voids and drift == 0,
            'void_reasons': v['reasons'], 'cpu_ok': cpu, 'void_files': voids, 'confirmed_drifts': drift,
            'checks_failed': [' '.join(c) for c in checks if c[0] != 'PASS'], 'checks': len(checks),
            'freq_ghz': [round(s['ghz'], 4) if 'ghz' in s else s.get('error') for s in freq], 'refs': ref,
            'machine': reports[0]['machine']}


def faults(rows, null_arm):
    devs = [abs(r['null'][f'{null_arm}_{m}']) for r in rows for m in MODES]
    unresolved = sum(bool(r['unresolved']) for r in rows)
    retried = sum(r['attempts'] > 1 for r in rows)
    spread = max(s for r in rows for s in r['spread'].values())
    return {'ok': unresolved == 0 and retried <= 2 and max(devs) <= 0.02, 'unresolved': unresolved,
            'retried': retried, 'max_null_dev': max(devs), 'max_round_spread': spread}


def vs_ffmpeg(rows, arm):
    out = {}
    for m, ff in (('discard', 'ff_null'), ('write', 'ff_raw')):
        v = {r['stream']: r['seconds'][f'{arm}_{m}'] / r['seconds'][ff] for r in rows}
        out[m] = {'all': statistics.median(v.values()), 'faster': sum(x < 1 for x in v.values()), 'n': len(v),
                  'class': by_class(v)}
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('es_json')
    ap.add_argument('aa')
    ap.add_argument('na')
    ap.add_argument('--json', help='write every number to this file')
    a = ap.parse_args()
    with open(a.es_json) as f:
        es = json.load(f)
    aa_rows, aa_rep = load(a.aa, es, ('a', 'b'), 'a')
    na_rows, na_rep = load(a.na, es, ('base', 'k'), 'base')
    h = {'AA': host(a.aa, aa_rep), 'NA': host(a.na, na_rep)}
    aa = ratio_stats(aa_rows, 'b', 'a')
    null = {}
    for m in MODES:
        v = {r['stream']: 1 + r['null'][f'base_{m}'] for r in na_rows}
        null[m] = {'all': med(list(v.values())), 'class': by_class(v)}
    f = {'AA': faults(aa_rows, 'a'), 'NA': faults(na_rows, 'base')}
    ff = {'AA': vs_ffmpeg(aa_rows, 'a'), 'NA': vs_ffmpeg(na_rows, 'base')}
    within = lambda s, lo, hi: lo <= s <= hi
    ref_gap = {n: h['NA']['refs'][n] / h['AA']['refs'][n] - 1 for n in h['AA']['refs']}
    ff_gap = ff['NA']['discard']['all'] / ff['AA']['discard']['all'] - 1
    bars = {
        'H-1': h['AA']['H-1'] and h['NA']['H-1'],
        'H-2': h['AA']['H-2'] and h['NA']['H-2'],
        'A-1': all(within(aa[m]['all']['median'], 0.998, 1.002) for m in MODES),
        'A-2': all(aa[m]['all']['half_width'] <= 0.0025 for m in MODES),
        'A-3': all(within(x, 0.995, 1.005) for m in MODES for x in aa[m]['class'].values()),
        'A-4': all(aa[m]['max_dev'] <= 0.02 for m in MODES),
        'N-1': all(within(null[m]['all']['median'], 0.998, 1.002) and null[m]['all']['half_width'] <= 0.0025
                   and all(within(x, 0.995, 1.005) for x in null[m]['class'].values()) for m in MODES),
        'N-2': f['AA']['ok'] and f['NA']['ok'],
        'V-1': all(abs(g) <= 0.02 for g in ref_gap.values()),
        'V-2': abs(ff_gap) <= 0.01,
    }
    result = {'bars': bars, 'accepted': all(bars[b] for b in ACCEPT), 'host': h, 'aa': aa, 'null': null,
              'faults': f, 'vs_ffmpeg': ff, 'k_over_base': ratio_stats(na_rows, 'k', 'base'),
              'reference_gap': ref_gap, 'ffmpeg_ratio_gap': ff_gap}
    for b, ok in bars.items():
        print(f'{b}: {"met" if ok else "MISSED"}')
    for m in MODES:
        s, n = aa[m]['all'], null[m]['all']
        print(f'AA b/a {m}: {s["median"]:.4f} [{s["low"]:.4f}, {s["high"]:.4f}] half-width {s["half_width"]:.2%}, '
              f'classes {", ".join(f"{c} {x:.4f}" for c, x in aa[m]["class"].items())}, max dev {aa[m]["max_dev"]:.2%}')
        print(f'NA null {m}: {n["median"]:.4f} [{n["low"]:.4f}, {n["high"]:.4f}] half-width {n["half_width"]:.2%}, '
              f'classes {", ".join(f"{c} {x:.4f}" for c, x in null[m]["class"].items())}')
    for run in ('AA', 'NA'):
        x = h[run]
        print(f'{run}: void {x["void_reasons"]}, CPU ok {x["cpu_ok"]}, voided calls {x["void_files"]}, '
              f'confirmed drifts {x["confirmed_drifts"]}, failed checks {x["checks_failed"]} of {x["checks"]}, '
              f'GHz {x["freq_ghz"]}, refs {", ".join(f"{k} {t:.3f}s" for k, t in x["refs"].items())}, faults {f[run]}')
        for m in MODES:
            v = ff[run][m]
            print(f'{run} X/FFmpeg {m}: {v["all"]:.3f} ({v["faster"]}/{v["n"]} faster), '
                  + ', '.join(f'{c} {t:.3f}' for c, t in v['class'].items()))
    print(f'reference gap NA/AA: {", ".join(f"{k} {g:+.2%}" for k, g in ref_gap.items())}; '
          f'X/FFmpeg discard gap {ff_gap:+.2%}')
    for m in MODES:
        s = result['k_over_base'][m]['all']
        print(f'k/base {m}: {s["median"]:.3f} [{s["low"]:.3f}, {s["high"]:.3f}]')
    print('accepted' if result['accepted'] else 'NOT accepted')
    if a.json:
        with open(a.json, 'w') as fh:
            json.dump(result, fh, indent=1)
    return 0 if result['accepted'] else 1


if __name__ == '__main__':
    sys.exit(main())
