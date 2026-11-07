#!/usr/bin/env python3
"""Summarize per-plane quantizer comparisons from encoder A/B JSON reports."""
import argparse
import json
import math
from pathlib import Path

PLANES = ('y', 'u', 'v', 'weighted')
QPS = ('22', '28', '34', '40')


def quality(scores, plane):
    values = [scores[p] for p in ('y', 'u', 'v')]
    if not all(isinstance(v, (int, float)) and math.isfinite(v) for v in values):
        raise ValueError('every plane needs finite PSNR')
    return (6 * values[0] + values[1] + values[2]) / 8 if plane == 'weighted' else scores[plane]


def curve(points):
    if len(points) != len(QPS):
        raise ValueError('comparison requires four QPs')
    points = sorted(points)
    x, rate = zip(*points)
    if not all(math.isfinite(q) and math.isfinite(r) and r > 0 for q, r in points):
        raise ValueError('invalid quality or rate')
    if any(x[i] >= x[i + 1] for i in range(3)):
        raise ValueError('duplicate quality points')
    y = [math.log(r) for r in rate]
    h = [x[i + 1] - x[i] for i in range(3)]
    d = [(y[i + 1] - y[i]) / h[i] for i in range(3)]
    slopes = [0.] * 4
    for i in range(1, 3):
        if d[i - 1] * d[i] <= 0:
            continue
        w1, w2 = 2 * h[i] + h[i - 1], h[i] + 2 * h[i - 1]
        slopes[i] = (w1 + w2) / (w1 / d[i - 1] + w2 / d[i])
    for at, a, b in ((0, 0, 1), (3, 2, 1)):
        value = ((2 * h[a] + h[b]) * d[a] - h[a] * d[b]) / (h[a] + h[b])
        if value * d[a] <= 0:
            value = 0.
        elif d[a] * d[b] < 0 and abs(value) > abs(3 * d[a]):
            value = 3 * d[a]
        slopes[at] = value
    return x, y, slopes


def integrate(c, low, high):
    x, y, slopes = c
    result = 0.
    for i in range(3):
        a, b = max(low, x[i]), min(high, x[i + 1])
        if a >= b:
            continue
        h = x[i + 1] - x[i]
        delta = (y[i + 1] - y[i]) / h
        quadratic = (3 * delta - 2 * slopes[i] - slopes[i + 1]) / h
        cubic = (slopes[i] + slopes[i + 1] - 2 * delta) / (h * h)

        def primitive(v):
            t = v - x[i]
            return y[i] * t + slopes[i] * t * t / 2 + quadratic * t**3 / 3 + cubic * t**4 / 4

        result += primitive(b) - primitive(a)
    return result


def bd_rate(reference, candidate):
    a, b = curve(reference), curve(candidate)
    low, high = max(a[0][0], b[0][0]), min(a[0][-1], b[0][-1])
    if high <= low:
        raise ValueError('no common quality interval')
    return 100 * math.expm1((integrate(b, low, high) - integrate(a, low, high)) / (high - low))


def summarize(record, baseline, candidate):
    cells = record['cells']
    if set(cells) != set(QPS):
        raise ValueError('missing or extra QP cells')
    result = {'label': record['label'], 'baseline': baseline, 'candidate': candidate,
              'bd_rate': {}, 'nonmonotone_rate_planes': []}
    for plane in PLANES:
        points = {}
        for arm in (baseline, candidate):
            points[arm] = []
            for qp in QPS:
                cell = cells[qp]['identity'][arm]
                if cell.get('ffmpeg_decode_matches') is not True:
                    raise ValueError('comparison requires matching oracle reconstruction')
                points[arm].append((quality(cell['psnr'], plane), cell['bytes']))
        result['bd_rate'][plane] = bd_rate(points[baseline], points[candidate])
        if any(any(b[1] <= a[1] for a, b in zip(sorted(values), sorted(values)[1:]))
               for values in points.values()):
            result['nonmonotone_rate_planes'].append(plane)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reports', nargs='+', type=Path)
    parser.add_argument('--baseline', required=True)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    rows = [summarize(json.loads(path.read_text()), args.baseline, args.candidate) for path in args.reports]
    if len({row['label'] for row in rows}) != len(rows):
        raise ValueError('duplicate input label')
    args.output.write_text(json.dumps({'weighted_psnr': '(6*Y+U+V)/8', 'inputs': rows}, indent=2) + '\n')


if __name__ == '__main__':
    main()
